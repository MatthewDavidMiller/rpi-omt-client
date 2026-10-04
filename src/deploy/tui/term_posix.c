/* Copyright (c) 2026 Matthew David Miller
 * SPDX-License-Identifier: MIT
 *
 * The POSIX terminal: termios raw mode on the controlling terminal.
 *
 * The terminal is restored on every exit path the process can take: a
 * normal return, a termination signal, and a crash. A crash handler that left
 * the console in raw mode on the alternate screen would make the operator's
 * shell unusable, which is worse than whatever went wrong.
 */
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "deploy/tui/term.h"

#define ENTER_SEQUENCE "\x1b[?1049h\x1b[?25l\x1b[2J"
#define LEAVE_SEQUENCE "\x1b[0m\x1b[?25h\x1b[?1049l"

static struct termios saved;
static volatile sig_atomic_t active;
static uint8_t pending[256];
static size_t pending_len;

static void write_all(const char *data, size_t len) {
    while (len > 0) {
        ssize_t n = write(1, data, len);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return;
        data += n;
        len -= (size_t)n;
    }
}

void term_leave(void) {
    if (!active) return;
    active = 0;
    tcsetattr(0, TCSAFLUSH, &saved);
    write_all(LEAVE_SEQUENCE, sizeof(LEAVE_SEQUENCE) - 1);
}

/* Only async-signal-safe calls: tcsetattr, write, signal, raise. */
static void on_fatal(int sig) {
    term_leave();
    signal(sig, SIG_DFL);
    raise(sig);
}

bool term_enter(dp_err *err) {
    if (!isatty(0) || !isatty(1)) {
        dp_fail(err, "rpi-omt-deploy-tui needs an interactive terminal; use rpi-omt-deploy for "
                     "scripts");
        return false;
    }
    if (tcgetattr(0, &saved) != 0) {
        dp_fail_os(err, "cannot read the terminal settings");
        return false;
    }
    struct termios raw = saved;
    raw.c_iflag &= ~(tcflag_t)(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    raw.c_oflag &= ~(tcflag_t)OPOST;
    raw.c_cflag |= CS8;
    /* No ISIG: Ctrl+C cancels a job rather than killing the program. */
    raw.c_lflag &= ~(tcflag_t)(ECHO | ICANON | IEXTEN | ISIG);
    raw.c_cc[VMIN] = 0;
    raw.c_cc[VTIME] = 0;
    if (tcsetattr(0, TCSAFLUSH, &raw) != 0) {
        dp_fail_os(err, "cannot switch the terminal to raw mode");
        return false;
    }
    active = 1;
    static const int fatal[] = {SIGSEGV, SIGBUS, SIGFPE, SIGILL, SIGABRT, SIGTERM, SIGHUP, SIGQUIT};
    for (size_t i = 0; i < OMT_ARRAY_LEN(fatal); i++) signal(fatal[i], on_fatal);
    write_all(ENTER_SEQUENCE, sizeof(ENTER_SEQUENCE) - 1);
    return true;
}

bool term_size(int *width, int *height) {
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) != 0 || ws.ws_col == 0) return false;
    *width = ws.ws_col;
    *height = ws.ws_row;
    return true;
}

void term_write(const char *data, size_t len) { write_all(data, len); }

static bool fill(uint32_t timeout_ms) {
    struct pollfd p = {0, POLLIN, 0};
    int ready = poll(&p, 1, (int)timeout_ms);
    if (ready <= 0) return false;
    ssize_t n = read(0, pending + pending_len, sizeof(pending) - pending_len);
    if (n <= 0) return false;
    pending_len += (size_t)n;
    return true;
}

int term_read_keys(key_event *out, int max, uint32_t timeout_ms) {
    if (pending_len == 0 && !fill(timeout_ms)) return 0;
    /* Drain whatever else arrived with it. */
    while (pending_len < sizeof(pending) && fill(0)) {}
    int count = 0;
    size_t used = term_decode(pending, pending_len, false, out, max, &count);
    if (used == 0 && count == 0 && pending_len > 0) {
        /* An escape that did not complete: a lone Esc, unless more of the
         * sequence arrives within a moment. */
        if (!fill(25)) used = term_decode(pending, pending_len, true, out, max, &count);
    }
    memmove(pending, pending + used, pending_len - used);
    pending_len -= used;
    return count;
}
