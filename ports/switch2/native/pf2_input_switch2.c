// SPDX-License-Identifier: MIT OR Apache-2.0
// Switch 2 native HID sampling. The official pf-nx pad_map remains authoritative
// for translating these samples into Punktfunk gamepad protocol button bits.
#include "pf2_native.h"
#if !defined(__SWITCH2__)
#error "The native HID backend is Switch 2-only"
#endif
#include <switch.h>
#include <stdlib.h>
#include <stdbool.h>
#include <stddef.h>

// ABI verified against the same libounce HID types Moonlight NX2 uses.
_Static_assert(sizeof(HidTouchState) == 40, "HidTouchState ABI");
_Static_assert(sizeof(HidTouchScreenState) == 656, "HidTouchScreenState ABI");
_Static_assert(offsetof(HidTouchScreenState, touches) == 16, "touch array ABI");
_Static_assert(sizeof(Pf2TouchRaw) == 16, "Pf2TouchRaw ABI");

struct Pf2Input {
    // Npad 1..4 are the console's Bluetooth/player assignments. Handheld
    // Joy-Cons are a separate source which joins player 1 only.
    PadState states[4];
    PadState handheld;
    bool handheld_connected;
    bool prefer_handheld_axes;
    HidAnalogStickState previous_handheld[2];
    HidAnalogStickState previous_player1[2];
    HidVibrationDeviceHandle motors[4][2];
    uint32_t motor_style[4];
    int motor_count[4];
    bool motor_ready[4];
    HidSixAxisSensorHandle sensors[4][2];
    uint32_t sensor_style[4];
    int sensor_count[4];
    bool connected[4];
    uint32_t styles[4];
};

// The Npad style identifies a Bluetooth controller. The separate handheld
// source may join P1, but must never hide a real P1 Pro Controller.
// Values are the stable protocol-neutral tags in Pf2PadRaw.
static uint32_t pad_style(PadState *pad) {
    uint32_t set = padGetStyleSet(pad);
    if (set & HidNpadStyleTag_NpadFullKey) return 5;
    if (set & HidNpadStyleTag_NpadJoyDual) return 2;
    if (set & HidNpadStyleTag_NpadJoyLeft) return 3;
    if (set & HidNpadStyleTag_NpadJoyRight) return 4;
    return 0;
}

static HidNpadStyleTag native_style(uint32_t style) {
    switch (style) {
        case 1: return HidNpadStyleTag_NpadHandheld;
        case 2: return HidNpadStyleTag_NpadJoyDual;
        case 3: return HidNpadStyleTag_NpadJoyLeft;
        case 4: return HidNpadStyleTag_NpadJoyRight;
        case 5: return HidNpadStyleTag_NpadFullKey;
        default: return 0;
    }
}

static HidNpadIdType pad_id(uint32_t slot, uint32_t style) {
    return style == 1 ? HidNpadIdType_Handheld : (HidNpadIdType)(HidNpadIdType_No1 + slot);
}

static void stop_motion(Pf2Input *p, uint32_t slot) {
    for (int i = 0; i < p->sensor_count[slot]; ++i)
        hidStopSixAxisSensor(p->sensors[slot][i]);
    p->sensor_count[slot] = 0;
    p->sensor_style[slot] = 0;
}

// The host's two rumble levels are LEFT and RIGHT actuators, not two bands
// that should both be played on every actuator. Match its Switch Pro / Joy-Con
// wire identity: a left Joy-Con has only the left actuator, and vice versa.
// Keep Moonlight NX2's SDK frequencies and 16-bit amplitude normalization.
static Result send_motor_levels(Pf2Input *p, uint32_t slot, uint16_t left, uint16_t right) {
    const HidVibrationValue levels[2] = {
        { .amp_low = (float)left / 65535.0f, .freq_low = 160.0f,
          .amp_high = 0.0f, .freq_high = 320.0f },
        { .amp_low = 0.0f, .freq_low = 160.0f,
          .amp_high = (float)right / 65535.0f, .freq_high = 320.0f },
    };
    if (!p->motor_ready[slot]) return 0;
    // An individual right Joy-Con owns one handle, but its actuator is RIGHT.
    if (p->motor_count[slot] == 1) {
        HidVibrationValue one = p->motor_style[slot] == 4 ? levels[1] : levels[0];
        return hidSendVibrationValues(p->motors[slot], &one, 1);
    }
    return hidSendVibrationValues(p->motors[slot], levels, p->motor_count[slot]);
}

// Keep the handle live on an SDK error so the next poll can retry the stop.
static bool stop_motors(Pf2Input *p, uint32_t slot) {
    return !p->motor_ready[slot] ||
        R_SUCCEEDED(send_motor_levels(p, slot, 0, 0));
}

// The input slot remains P1 for Pro and attached Joy-Cons. Motor output must
// follow whichever physical pad currently owns P1's axes, never drive both.
static uint32_t active_motor_style(const Pf2Input *p, uint32_t slot) {
    if (slot == 0 && p->handheld_connected &&
        (p->styles[0] == 1 || p->prefer_handheld_axes)) return 1;
    return p->styles[slot];
}

Pf2Error pf2_input_open(Pf2Input **out) {
    if (!out) return PF2_INVALID;
    *out = NULL;
    Pf2Input *p = calloc(1, sizeof(*p));
    if (!p) return PF2_NO_MEMORY;
    padConfigureInput(4, HidNpadStyleSet_NpadStandard);
    padInitialize(&p->states[0], HidNpadIdType_No1);
    padInitialize(&p->handheld, HidNpadIdType_Handheld);
    padInitialize(&p->states[1], HidNpadIdType_No2);
    padInitialize(&p->states[2], HidNpadIdType_No3);
    padInitialize(&p->states[3], HidNpadIdType_No4);
    hidInitializeTouchScreen();
    *out = p;
    return PF2_OK;
}

// No input-thread allocations and no additional HID IPC: the original
// padInitializeDefault also reads handheld + No1, just into the same PadState.
static bool stick_activity(HidAnalogStickState now, HidAnalogStickState before) {
    // Reject normal stick-centre jitter. Only actual user movement transfers
    // the analogue controls between attached Joy-Cons and the Pro Controller.
    const int32_t deadzone = 3200;
    const int32_t change = 700;
    // Crossing the deadzone also counts: a slowly moved stick must not be
    // ignored just because each 4-ms delta is smaller than `change`.
    const bool entered =
        ((now.x > deadzone || now.x < -deadzone) &&
         (before.x <= deadzone && before.x >= -deadzone)) ||
        ((now.y > deadzone || now.y < -deadzone) &&
         (before.y <= deadzone && before.y >= -deadzone));
    const bool moved =
        (abs(now.x) > deadzone || abs(now.y) > deadzone) &&
        (abs(now.x - before.x) >= change ||
         abs(now.y - before.y) >= change);
    return entered || moved;
}

static bool pad_activity(const PadState *pad, HidAnalogStickState previous[2]) {
    // The stick-direction pseudo-buttons (bits 16..23) can chatter near an
    // angle boundary; they must not continually steal focus from another pad.
    const uint64_t physical_buttons = UINT64_C(0xffff) | (UINT64_C(0xf) << 24);
    bool moved = ((padGetButtonsDown(pad) & physical_buttons) != 0);
    for (unsigned j = 0; j < 2; ++j) {
        HidAnalogStickState now = padGetStickPos(pad, j);
        moved |= stick_activity(now, previous[j]);
        previous[j] = now;
    }
    return moved;
}

Pf2Error pf2_input_poll(Pf2Input *p, Pf2PadRaw out_pads[4]) {
    if (!p || !out_pads) return PF2_INVALID;
    padUpdate(&p->handheld);
    const bool handheld = padIsHandheld(&p->handheld) && padIsConnected(&p->handheld);
    p->handheld_connected = handheld;
    for (unsigned i = 0; i < 4; i++) {
        PadState *pad = &p->states[i];
        padUpdate(pad);
        Pf2PadRaw *sample = &out_pads[i];
        *sample = (Pf2PadRaw) {.slot = i};
        const bool player = padIsConnected(pad);
        // The physically attached Joy-Cons are an additional P1 input; Joy-Cons
        // assigned via Bluetooth remain at the console's No1..No4 IDs.
        if (!player && !(i == 0 && handheld)) {
            if (stop_motors(p, i)) p->motor_ready[i] = false;
            stop_motion(p, i);
            p->connected[i] = false;
            p->styles[i] = 0;
            if (i == 0) {
                p->prefer_handheld_axes = false;
                p->previous_handheld[0] = p->previous_handheld[1] = (HidAnalogStickState){0};
                p->previous_player1[0] = p->previous_player1[1] = (HidAnalogStickState){0};
            }
            continue; // Release held buttons on disconnect.
        }
        uint32_t style = player ? pad_style(pad) : 1;
        if (i == 0) {
            // A Pro Controller connected as No1 retains the SwitchPro identity,
            // even while the physically attached Joy-Cons are also active.
            // Avoid replugging the host pad each time a user changes controls.
            bool hand_moved = handheld && pad_activity(&p->handheld, p->previous_handheld);
            bool player_moved = player && pad_activity(pad, p->previous_player1);
            if (!handheld) {
                p->prefer_handheld_axes = false;
                p->previous_handheld[0] = p->previous_handheld[1] = (HidAnalogStickState){0};
            } else if (!player) {
                p->prefer_handheld_axes = true;
                p->previous_player1[0] = p->previous_player1[1] = (HidAnalogStickState){0};
            } else if (player_moved) {
                p->prefer_handheld_axes = false;
            } else if (hand_moved) {
                p->prefer_handheld_axes = true;
            }
        }
        if (p->styles[i] != style) {
            // Do not reuse vibration/IMU handles across a physical style change.
            if (stop_motors(p, i)) p->motor_ready[i] = false;
            stop_motion(p, i);
        }
        p->styles[i] = style;
        p->connected[i] = true;
        // A change in the active P1 source must stop the *previous* physical
        // actuator even if the host sends no new rumble event during the swap.
        if (p->motor_ready[i] && p->motor_style[i] != active_motor_style(p, i)) {
            if (stop_motors(p, i)) p->motor_ready[i] = false;
        }
        PadState *axes = pad;
        if (i == 0 && handheld) {
            sample->buttons = padGetButtons(&p->handheld) |
                (player ? padGetButtons(pad) : 0);
            if (!player || p->prefer_handheld_axes) axes = &p->handheld;
        } else {
            sample->buttons = padGetButtons(pad);
        }
        HidAnalogStickState l = padGetStickPos(axes, 0);
        HidAnalogStickState r = padGetStickPos(axes, 1);
        sample->connected = 1;
        sample->style = style;
        sample->left_x = l.x;
        sample->left_y = l.y;
        sample->right_x = r.x;
        sample->right_y = r.y;
    }
    return PF2_OK;
}

// The input thread logs source transitions, rather than polling every frame.
uint32_t pf2_input_handheld_attached(const Pf2Input *p) {
    return p && p->handheld_connected ? 1u : 0u;
}

Pf2Error pf2_input_motion(Pf2Input *p, uint32_t slot, Pf2MotionRaw *out) {
    if (!p || !out || slot >= 4) return PF2_INVALID;
    *out = (Pf2MotionRaw){0};
    if (!p->connected[slot]) return PF2_NOT_READY;
    uint32_t style = p->styles[slot];
    HidNpadStyleTag tag = native_style(style);
    if (!tag) return PF2_UNSUPPORTED;
    if (p->sensor_style[slot] != style || !p->sensor_count[slot]) {
        stop_motion(p, slot);
        int count = style == 2 ? 2 : 1;
        HidNpadIdType id = pad_id(slot, style);
        Result rc = hidGetSixAxisSensorHandles(p->sensors[slot], count, id, tag);
        if (R_FAILED(rc)) return PF2_UNSUPPORTED;
        int started = 0;
        for (; started < count; ++started) {
            if (R_FAILED(hidStartSixAxisSensor(p->sensors[slot][started]))) break;
        }
        if (started != count) {
            for (int i = 0; i < started; ++i) hidStopSixAxisSensor(p->sensors[slot][i]);
            return PF2_SDK_ERROR;
        }
        p->sensor_style[slot] = style;
        p->sensor_count[slot] = count;
    }
    HidSixAxisSensorState state = {0};
    // For paired Joy-Cons use the right-hand IMU, matching Moonlight NX2.
    if (hidGetSixAxisSensorStates(p->sensors[slot][p->sensor_count[slot]-1], &state, 1) == 0)
        return PF2_NOT_READY;
    out->accel[0] = state.acceleration.x;
    out->accel[1] = state.acceleration.y;
    out->accel[2] = state.acceleration.z;
    out->gyro[0] = state.angular_velocity.x;
    out->gyro[1] = state.angular_velocity.y;
    out->gyro[2] = state.angular_velocity.z;
    return PF2_OK;
}

Pf2Error pf2_input_touches(Pf2Input *p, Pf2TouchRaw out_touches[16], uint32_t *count) {
    if (!p || !out_touches || !count) return PF2_INVALID;
    *count = 0;
    HidTouchScreenState state = {0};
    // Same API as Moonlight NX2. A zero result means no new HID sample.
    if (!hidGetTouchScreenStates(&state, 1)) return PF2_OK;
    uint32_t n = state.count > 0 ? (uint32_t)state.count : 0;
    if (n > 16) n = 16;
    for (uint32_t i = 0; i < n; i++) {
        const HidTouchState *t = &state.touches[i];
        out_touches[i] = (Pf2TouchRaw) {
            .finger_id = t->finger_id, .attributes = t->attributes,
            .x = t->x, .y = t->y,
        };
    }
    *count = n;
    return PF2_OK;
}

Pf2Error pf2_input_rumble(Pf2Input *p, uint32_t slot, uint16_t low, uint16_t high) {
    if (!p || slot >= 4) return PF2_INVALID;
    if (!p->connected[slot] && (low || high)) return PF2_NOT_READY;
    uint32_t style = active_motor_style(p, slot);
    HidNpadStyleTag tag = native_style(style);
    if (p->motor_ready[slot] && p->motor_style[slot] != style) {
        if (!stop_motors(p, slot)) return PF2_SDK_ERROR;
        p->motor_ready[slot] = false;
    }
    if (!p->motor_ready[slot]) {
        if (!low && !high) return PF2_OK;
        if (!tag) return PF2_UNSUPPORTED;
        int count = style == 3 || style == 4 ? 1 : 2;
        Result rc = hidInitializeVibrationDevices(p->motors[slot], count, pad_id(slot, style), tag);
        if (R_FAILED(rc)) return PF2_UNSUPPORTED;
        p->motor_ready[slot] = true;
        p->motor_style[slot] = style;
        p->motor_count[slot] = count;
    }
    Result rc = send_motor_levels(p, slot, low, high);
    return R_SUCCEEDED(rc) ? PF2_OK : PF2_SDK_ERROR;
}

void pf2_input_release_all(Pf2Input *p) {
    if (!p) return;
    for (uint32_t i = 0; i < 4; i++) stop_motors(p, i);
}

void pf2_input_close(Pf2Input *p) {
    pf2_input_release_all(p);
    for (uint32_t i = 0; i < 4; ++i) stop_motion(p, i);
    free(p);
}
