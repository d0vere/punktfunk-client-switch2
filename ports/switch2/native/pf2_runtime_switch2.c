// SPDX-License-Identifier: MIT OR Apache-2.0
// Title-wide socket lifecycle and applet events; all used symbols verified
// against ouncebrew/libounce headers. Avoids shutting down Quinn mid-session.
#include "pf2_native.h"
#if !defined(__SWITCH2__)
#error "Switch 2 runtime required"
#endif
#include <switch.h>
#include <stdlib.h>
#include <stdbool.h>
#include <errno.h>
#include <string.h>
#include <unistd.h>
#include <stdio.h>
#include <time.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/ioctl.h>
#include <netinet/in.h>

// Pre-logger breadcrumbs are stored OUTSIDE the app data directory so a
// second-launch storage failure can still be investigated without deleting
// the client's pairing identity or settings.
void pf2_runtime_boot_trace(const char *stage) {
    if (!stage) return;
    const char *path="sdmc:/switch/punktfunk-boot.log";
    const char *previous="sdmc:/switch/punktfunk-boot.log.1";
    // Rotate at 512 KiB, preserving one earlier boot trace. This function
    // runs during boot and recovery, never from a real-time audio callback.
    struct stat info;
    if (stat(path,&info)==0 && info.st_size >= 512 * 1024) {
        remove(previous);
        rename(path,previous);
    }
    FILE *f = fopen(path, "a");
    if (!f) return; // SD may not be mounted yet; no safe logging sink here.
    const time_t now = time(NULL);
    fprintf(f, "%lld %s\n", (long long)now, stage);
    fflush(f);
    fclose(f);
}

// The console's SD filesystem can refuse recursive mkdir on an existing
// directory. Do not mistake EEXIST for a fatal second-launch error.
static Pf2Error pf2_mkdir_existing_ok(const char *path, const char *failure_tag) {
    if (mkdir(path, 0777) == 0 || errno == EEXIST) return PF2_OK;
    const int err = errno;
    char diagnostic[128];
    snprintf(diagnostic, sizeof(diagnostic), "%s errno=%d", failure_tag, err);
    pf2_runtime_boot_trace(diagnostic);
    return PF2_SDK_ERROR;
}

Pf2Error pf2_runtime_prepare_storage(void) {
    Pf2Error rc = pf2_mkdir_existing_ok("sdmc:/switch/punktfunk", "storage.data.mkdir.failed");
    if (rc != PF2_OK) return rc;
    return pf2_mkdir_existing_ok("sdmc:/switch/punktfunk/logs", "storage.logs.mkdir.failed");
}

struct Pf2Runtime {
    bool networking;
    bool exit_locked;
    bool closing;
    bool close_sent;
    bool boosted_socket_buffers;
    Result boost_result;
    uint32_t udp_tx_bytes, udp_rx_bytes;
    AppletOperationMode mode;
    AppletFocusState focus;
};

// libounce's socketInitializeDefault configures only 0x2400 (9 KiB) of UDP
// send transfer memory. That matches the SO_SNDBUF=9 KiB observed in the
// Punktfunk QUIC log. Try larger BSD transfer buffers *before* any socket is
// created. Keep the SDK-default fallback if the service refuses the request.
// This is a per-process service configuration, not Linux sysctl tuning.
#define PF2_BSD_UDP_TX_BYTES (256u * 1024u)
#define PF2_BSD_UDP_RX_BYTES (1024u * 1024u)

static Result pf2_init_bsd_network(Pf2Runtime *r) {
    SocketInitConfig selected = *socketGetDefaultInitConfig();
    selected.udp_tx_buf_size = PF2_BSD_UDP_TX_BYTES;
    selected.udp_rx_buf_size = PF2_BSD_UDP_RX_BYTES;
    Result rc = socketInitialize(&selected);
    r->boost_result = rc;
    if (R_SUCCEEDED(rc)) {
        r->boosted_socket_buffers = true;
    } else {
        // socketInitialize rolls back its partially opened BSD session.
        // We need a working network even on a firmware that rejects tuning.
        selected = *socketGetDefaultInitConfig();
        rc = socketInitializeDefault();
    }
    if (R_SUCCEEDED(rc)) {
        r->udp_tx_bytes = selected.udp_tx_buf_size;
        r->udp_rx_bytes = selected.udp_rx_buf_size;
    }
    return rc;
}

Pf2Error pf2_runtime_open(Pf2Runtime **out) {
    if (!out) return PF2_INVALID;
    *out = NULL;
    Pf2Runtime *r = calloc(1, sizeof(*r));
    if (!r) return PF2_NO_MEMORY;
    Result rc = pf2_init_bsd_network(r);
    if (R_FAILED(rc)) { free(r); return PF2_SDK_ERROR; }
    r->networking = true;
    r->mode = appletGetOperationMode();
    r->focus = appletGetFocusState();
    rc = appletLockExit();
    if (R_FAILED(rc)) { socketExit(); free(r); return PF2_SDK_ERROR; }
    r->exit_locked = true;
    *out = r;
    return PF2_OK;
}

Pf2Error pf2_runtime_socket_tuning(const Pf2Runtime *r, Pf2SocketTuning *out) {
    if (!r || !r->networking || !out) return PF2_INVALID;
    memset(out, 0, sizeof(*out));
    out->boosted = r->boosted_socket_buffers ? 1u : 0u;
    out->boost_result = (uint32_t)r->boost_result;
    out->requested_tx = r->udp_tx_bytes;
    out->requested_rx = r->udp_rx_bytes;
    out->socket_sndbuf = -1;
    out->socket_rcvbuf = -1;
    // Sample a newly opened socket's *actual* BSD buffer sizes. The real
    // QUIC socket can further adjust these; its values remain in the core log.
    int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    if (fd >= 0) {
        socklen_t length = sizeof(out->socket_sndbuf);
        if (getsockopt(fd, SOL_SOCKET, SO_SNDBUF, &out->socket_sndbuf, &length) < 0)
            out->socket_sndbuf = -1;
        length = sizeof(out->socket_rcvbuf);
        if (getsockopt(fd, SOL_SOCKET, SO_RCVBUF, &out->socket_rcvbuf, &length) < 0)
            out->socket_rcvbuf = -1;
        close(fd);
    }
    return PF2_OK;
}

Pf2Error pf2_runtime_net_probe(const Pf2Runtime *r, Pf2NetProbe *out) {
    if (!r || !r->networking || !out) return PF2_INVALID;
    memset(out, 0, sizeof(*out));
    out->bind_rc = -2;
    out->nonblock_rc = -2;

    int fd = socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP);
    out->socket_rc = fd < 0 ? -1 : 0;
    out->socket_errno = fd < 0 ? errno : 0;
    out->socket_bsd_result = socketGetLastResult();
    if (fd < 0) return PF2_OK;

    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_len = sizeof(addr); // libounce uses FreeBSD sockaddr layout.
    addr.sin_addr.s_addr = INADDR_ANY;
    int rc = bind(fd, (const struct sockaddr *)&addr, sizeof(addr));
    out->bind_rc = rc;
    out->bind_errno = rc < 0 ? errno : 0;
    out->bind_bsd_result = socketGetLastResult();
    if (rc == 0) {
        int enabled = 1;
        rc = ioctl(fd, FIONBIO, &enabled);
        out->nonblock_rc = rc;
        out->nonblock_errno = rc < 0 ? errno : 0;
        out->nonblock_bsd_result = socketGetLastResult();
    }
    close(fd);
    return PF2_OK;
}

Pf2Error pf2_runtime_nonblock_fd(int32_t fd, int32_t *ioctl_rc,
                                  int32_t *ioctl_errno, uint32_t *bsd_result) {
    if (fd < 0 || !ioctl_rc || !ioctl_errno || !bsd_result) return PF2_INVALID;
    int enabled = 1;
    int rc = ioctl(fd, FIONBIO, &enabled);
    *ioctl_rc = rc;
    *ioctl_errno = rc < 0 ? errno : 0;
    *bsd_result = socketGetLastResult();
    return PF2_OK;
}

Pf2Error pf2_runtime_poll(Pf2Runtime *r, uint32_t *flags, uint32_t *docked) {
    if (!r || !flags || !docked) return PF2_INVALID;
    *flags = 0;
    // appletMainLoop pumps Horizon events; false means exit request.
    if (!appletMainLoop()) {
        r->closing = true;
        if (!r->close_sent) { *flags |= 4 | 32; r->close_sent = true; }
    }
    const AppletFocusState focus = appletGetFocusState();
    if (focus != r->focus) {
        if (focus == AppletFocusState_InFocus) {
            *flags |= (r->focus == AppletFocusState_Background ? 8 : 2);
        } else if (focus == AppletFocusState_Background) {
            *flags |= 4;
        } else {
            *flags |= 1;
        }
        r->focus = focus;
    }
    const AppletOperationMode mode = appletGetOperationMode();
    if (mode != r->mode) { *flags |= 16; r->mode = mode; }
    *docked = mode == AppletOperationMode_Console;
    return PF2_OK;
}

uint32_t pf2_runtime_docked(const Pf2Runtime *r) {
    return r && r->mode == AppletOperationMode_Console;
}

uint32_t pf2_runtime_applet(const Pf2Runtime *r) {
    if (!r) return 0;
    AppletType type = appletGetAppletType();
    return type != AppletType_Application && type != AppletType_SystemApplication;
}

void pf2_runtime_close(Pf2Runtime *r) {
    if (!r) return;
    if (r->exit_locked) appletUnlockExit();
    if (r->networking) socketExit();
    free(r);
}
