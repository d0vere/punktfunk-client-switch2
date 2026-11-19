/*
 * Limited POSIX file-mapping compatibility for Skia/ICU on Switch 2 newlib.
 * Adapted from the official pinned Punktfunk client-switch
 * toolchain/skia/compat.c (commit a332988f7509bd4c6fbde7252bd1e2b874019f45).
 * No Moonlight/Azkali code.
 *
 * Only read-only private file mappings are supported. The implementation
 * returns an allocated copy, not OS virtual memory; it must NOT be used as a
 * general-purpose mmap substitute. A process-wide C11 atomic lock makes the shim's
 * own positional reads mutually exclusive, but this is NOT a full POSIX pread:
 * external code calling read/lseek on the same open-file description remains
 * unsafe. Do not share these fds with unrestricted file-position consumers.
 */
#if !defined(__SWITCH2__)
#error "Skia compatibility shim is restricted to the Switch 2 build"
#endif

#include <errno.h>
#include <stdint.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/types.h>
#include <unistd.h>

// All Skia/ICU calls through this shim serialize seek/read/restore.
// Protect shared open-file descriptions, not just individual raw fd numbers.
static atomic_flag pf2_pread_lock = ATOMIC_FLAG_INIT;
static void pf2_pread_lock_acquire(void) {
    // No libpthread linker dependency: ounce-devkit exposes C11 atomics.
    // Only Skia/ICU shim positional reads take this short critical section.
    while (atomic_flag_test_and_set_explicit(&pf2_pread_lock, memory_order_acquire)) {
#if defined(__aarch64__)
        __asm__ volatile("yield" ::: "memory");
#endif
    }
}
static void pf2_pread_lock_release(void) {
    atomic_flag_clear_explicit(&pf2_pread_lock, memory_order_release);
}

/* A strong reference to this symbol from the Rust platform ensures the
 * archive member is extracted, even if link-order changes in rust-lld. */
void pf2_skia_compat_link_guard(void) { }

/* Newlib's POSIX pread stub is absent on this target. Restore the descriptor's
 * offset, preserving the original read errno if the read itself failed. */
ssize_t pf2_skia_file_pread(int fd, void *buf, size_t len, off_t off) {
    if (off < 0) {
        errno = EINVAL;
        return -1;
    }
    pf2_pread_lock_acquire();
    const off_t original = lseek(fd, 0, SEEK_CUR);
    if (original == (off_t)-1) {
        const int err=errno;
        pf2_pread_lock_release();
        errno=err;
        return -1;
    }
    if (lseek(fd, off, SEEK_SET) == (off_t)-1) {
        const int err=errno;
        pf2_pread_lock_release();
        errno=err;
        return -1;
    }
    const ssize_t got = read(fd, buf, len);
    const int saved_errno = errno;
    const int restored = lseek(fd, original, SEEK_SET) == (off_t)-1 ? errno : 0;
    pf2_pread_lock_release();
    if (restored != 0) { errno=restored; return -1; }
    if (got < 0) errno = saved_errno;
    return got;
}

/* Skia and ICU use this as a read-only file view. Don't silently grant write
 * mappings or shared mappings; those semantics cannot be emulated by malloc. */
void *pf2_skia_readonly_mmap(void *addr, size_t len, int prot,
                                 int flags, int fd, off_t off) {
    (void)addr;
    if (len == 0 || off < 0) {
        errno = EINVAL;
        return MAP_FAILED;
    }
    if ((prot & PROT_READ) == 0 || (prot & ~PROT_READ) != 0 ||
        (flags & MAP_PRIVATE) == 0 || (flags & MAP_SHARED) != 0) {
        errno = ENOSYS;
        return MAP_FAILED;
    }
    void *p = malloc(len);
    if (!p) {
        errno = ENOMEM;
        return MAP_FAILED;
    }
    /* Match the official shim's limited full-length-read contract. */
    const ssize_t n = pf2_skia_file_pread(fd, p, len, off);
    if (n < 0 || (size_t)n != len) {
        const int fail_errno = n < 0 ? errno : EIO;
        free(p);
        errno = fail_errno;
        return MAP_FAILED;
    }
    return p;
}

int pf2_skia_readonly_munmap(void *addr, size_t len) {
    (void)len;
    free(addr);
    return 0;
}

/* Public weak POSIX names are the exact symbols rust-lld said were absent. */
__attribute__((weak)) ssize_t pread(int fd, void *buf, size_t len, off_t off) {
    return pf2_skia_file_pread(fd, buf, len, off);
}

__attribute__((weak)) void *mmap(void *addr, size_t len, int prot,
                                 int flags, int fd, off_t off) {
    return pf2_skia_readonly_mmap(addr, len, prot, flags, fd, off);
}

__attribute__((weak)) int munmap(void *addr, size_t len) {
    return pf2_skia_readonly_munmap(addr, len);
}

__attribute__((weak)) int posix_madvise(void *addr, size_t len, int advice) {
    (void)addr;
    (void)len;
    (void)advice;
    return 0;
}
