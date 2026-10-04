/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The Windows console: output through virtual-terminal processing (Windows
 * 10 and later, conhost and Windows Terminal alike), input as console key
 * records decoded by virtual-key code, which reports F-keys and Ctrl+arrows
 * the same on every console host.
 */
#include <string.h>
#include <windows.h>

#include "deploy/tui/term.h"

#define ENTER_SEQUENCE "\x1b[?1049h\x1b[?25l\x1b[2J"
#define LEAVE_SEQUENCE "\x1b[0m\x1b[?25h\x1b[?1049l"

static HANDLE in_handle = INVALID_HANDLE_VALUE, out_handle = INVALID_HANDLE_VALUE;
static DWORD saved_in, saved_out;
static UINT saved_output_cp;
static volatile LONG active;
static WCHAR high_surrogate;

void term_write(const char *data, size_t len) {
    while (len > 0) {
        DWORD wrote = 0;
        DWORD want = len > (1u << 20) ? (1u << 20) : (DWORD)len;
        if (!WriteFile(out_handle, data, want, &wrote, NULL) || wrote == 0) return;
        data += wrote;
        len -= wrote;
    }
}

void term_leave(void) {
    if (InterlockedExchange(&active, 0) == 0) return;
    term_write(LEAVE_SEQUENCE, sizeof(LEAVE_SEQUENCE) - 1);
    SetConsoleMode(in_handle, saved_in);
    SetConsoleMode(out_handle, saved_out);
    if (saved_output_cp) SetConsoleOutputCP(saved_output_cp);
}

/* Closing the console window or logging off ends the process; restore the
 * console first so a shared console is left usable. */
static BOOL WINAPI on_console_event(DWORD type) {
    if (type == CTRL_CLOSE_EVENT || type == CTRL_LOGOFF_EVENT || type == CTRL_SHUTDOWN_EVENT) {
        term_leave();
    }
    return FALSE;
}

bool term_enter(dp_err *err) {
    in_handle = GetStdHandle(STD_INPUT_HANDLE);
    out_handle = GetStdHandle(STD_OUTPUT_HANDLE);
    if (!GetConsoleMode(in_handle, &saved_in) || !GetConsoleMode(out_handle, &saved_out)) {
        dp_fail(err, "rpi-omt-deploy-tui needs an interactive console; use rpi-omt-deploy for "
                     "scripts");
        return false;
    }
    DWORD out_mode = saved_out | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING |
                     DISABLE_NEWLINE_AUTO_RETURN;
    if (!SetConsoleMode(out_handle, out_mode)) {
        dp_fail(err, "this console does not support virtual-terminal output; use Windows "
                     "Terminal or Windows 10 or later");
        return false;
    }
    /* No processed input: Ctrl+C arrives as a key and cancels a job rather
     * than killing the program. No line input or echo. */
    DWORD in_mode = ENABLE_WINDOW_INPUT | ENABLE_EXTENDED_FLAGS;
    SetConsoleMode(in_handle, in_mode);
    saved_output_cp = GetConsoleOutputCP();
    SetConsoleOutputCP(CP_UTF8);
    SetConsoleCtrlHandler(on_console_event, TRUE);
    InterlockedExchange(&active, 1);
    term_write(ENTER_SEQUENCE, sizeof(ENTER_SEQUENCE) - 1);
    return true;
}

bool term_size(int *width, int *height) {
    CONSOLE_SCREEN_BUFFER_INFO info;
    if (!GetConsoleScreenBufferInfo(out_handle, &info)) return false;
    *width = info.srWindow.Right - info.srWindow.Left + 1;
    *height = info.srWindow.Bottom - info.srWindow.Top + 1;
    return *width > 0 && *height > 0;
}

/* The decoder is shared with POSIX; on Windows input arrives as key records
 * and is mapped directly. */
static bool map_key(const KEY_EVENT_RECORD *k, key_event *ev) {
    bool ctrl = (k->dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED)) != 0;
    bool alt = (k->dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED)) != 0;
    bool shift = (k->dwControlKeyState & SHIFT_PRESSED) != 0;
    ev->ch = 0;
    ev->f = 0;
    ev->ctrl = ctrl;
    switch (k->wVirtualKeyCode) {
    case VK_UP: ev->code = KEY_UP; return true;
    case VK_DOWN: ev->code = KEY_DOWN; return true;
    case VK_LEFT: ev->code = KEY_LEFT; return true;
    case VK_RIGHT: ev->code = KEY_RIGHT; return true;
    case VK_HOME: ev->code = KEY_HOME; return true;
    case VK_END: ev->code = KEY_END; return true;
    case VK_PRIOR: ev->code = KEY_PAGE_UP; return true;
    case VK_NEXT: ev->code = KEY_PAGE_DOWN; return true;
    case VK_DELETE: ev->code = KEY_DELETE; return true;
    case VK_BACK: ev->code = KEY_BACKSPACE; return true;
    case VK_TAB: ev->code = shift ? KEY_BACKTAB : KEY_TAB; return true;
    case VK_RETURN: ev->code = KEY_ENTER; return true;
    case VK_ESCAPE: ev->code = KEY_ESC; return true;
    default: break;
    }
    if (k->wVirtualKeyCode >= VK_F1 && k->wVirtualKeyCode <= VK_F12) {
        ev->code = KEY_F;
        ev->f = k->wVirtualKeyCode - VK_F1 + 1;
        return true;
    }
    /* Ctrl+letter, but not AltGr (reported as Ctrl+Alt), which types
     * characters on many layouts. */
    if (ctrl && !alt && k->wVirtualKeyCode >= 'A' && k->wVirtualKeyCode <= 'Z') {
        ev->code = KEY_CHAR;
        ev->ch = (uint32_t)(k->wVirtualKeyCode - 'A' + 'a');
        return true;
    }
    WCHAR c = k->uChar.UnicodeChar;
    if (c == 0) return false;
    if (c >= 0xD800 && c <= 0xDBFF) {
        high_surrogate = c;
        return false;
    }
    uint32_t cp = c;
    if (c >= 0xDC00 && c <= 0xDFFF) {
        if (!high_surrogate) return false;
        cp = 0x10000u + (((uint32_t)high_surrogate - 0xD800u) << 10) + ((uint32_t)c - 0xDC00u);
        high_surrogate = 0;
    }
    if (cp < 0x20) return false;
    ev->code = KEY_CHAR;
    ev->ch = cp;
    ev->ctrl = false;
    return true;
}

int term_read_keys(key_event *out, int max, uint32_t timeout_ms) {
    if (WaitForSingleObject(in_handle, timeout_ms) != WAIT_OBJECT_0) return 0;
    int count = 0;
    while (count < max) {
        DWORD available = 0;
        if (!GetNumberOfConsoleInputEvents(in_handle, &available) || available == 0) break;
        INPUT_RECORD records[32];
        DWORD got = 0;
        if (!ReadConsoleInputW(in_handle, records, 32, &got)) break;
        for (DWORD i = 0; i < got; i++) {
            if (records[i].EventType != KEY_EVENT || !records[i].Event.KeyEvent.bKeyDown) continue;
            key_event ev;
            if (!map_key(&records[i].Event.KeyEvent, &ev)) continue;
            WORD repeat = records[i].Event.KeyEvent.wRepeatCount;
            for (WORD r = 0; r < (repeat ? repeat : 1) && count < max; r++) out[count++] = ev;
        }
    }
    return count;
}
