#ifndef _WIN32
#define _POSIX_C_SOURCE 200809L
#endif
#include "deployer.h"

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <bcrypt.h>
#else
#include <sys/random.h>
#include <unistd.h>
#endif

static void set_error(char *error, size_t size, const char *message) {
    if (error != NULL && size > 0U) {
        (void)snprintf(error, size, "%s", message);
    }
}

static bool ascii_token(const char *value, const char *extra) {
    const unsigned char *p = (const unsigned char *)value;
    if (value == NULL || *value == '\0') {
        return false;
    }
    for (; *p != 0U; ++p) {
        if (!((*p >= (unsigned char)'a' && *p <= (unsigned char)'z') ||
              (*p >= (unsigned char)'A' && *p <= (unsigned char)'Z') ||
              (*p >= (unsigned char)'0' && *p <= (unsigned char)'9') ||
              strchr(extra, (int)*p) != NULL)) {
            return false;
        }
    }
    return true;
}

static bool contains_control(const char *value) {
    const unsigned char *p = (const unsigned char *)value;
    if (value == NULL) {
        return true;
    }
    for (; *p != 0U; ++p) {
        if (*p < 0x20U || *p == 0x7fU) {
            return true;
        }
    }
    return false;
}

static bool valid_utf8(const char *value) {
    const unsigned char *p = (const unsigned char *)value;
    while (*p != 0U) {
        uint32_t scalar;
        uint32_t minimum = 0U;
        size_t continuation = 0U;
        if (*p < 0x80U) {
            scalar = *p;
        } else if ((*p & 0xe0U) == 0xc0U) {
            continuation = 1U; scalar = *p & 0x1fU; minimum = 0x80U;
        } else if ((*p & 0xf0U) == 0xe0U) {
            continuation = 2U; scalar = *p & 0x0fU; minimum = 0x800U;
        } else if ((*p & 0xf8U) == 0xf0U) {
            continuation = 3U; scalar = *p & 0x07U; minimum = 0x10000U;
        } else {
            return false;
        }
        for (size_t i = 0U; i < continuation; ++i) {
            if (p[i + 1U] == 0U || (p[i + 1U] & 0xc0U) != 0x80U) {
                return false;
            }
            scalar = (scalar << 6U) | (p[i + 1U] & 0x3fU);
        }
        if ((continuation > 0U && scalar < minimum) || scalar > 0x10ffffU ||
            (scalar >= 0xd800U && scalar <= 0xdfffU)) {
            return false;
        }
        p += continuation + 1U;
    }
    return true;
}

static bool valid_manifest_name(const char *name) {
    size_t length;
    const char *start;
    if (name == NULL) return false;
    length = strlen(name);
    if (length == 0U || length > 240U || name[0] == '/' || name[length - 1U] == '/' ||
        strstr(name, "//") != NULL || !ascii_token(name, "._-/")) return false;
    start = name;
    for (;;) {
        const char *end = strchr(start, '/');
        const size_t count = end == NULL ? strlen(start) : (size_t)(end - start);
        if (count == 0U || (count == 1U && start[0] == '.') ||
            (count == 2U && start[0] == '.' && start[1] == '.')) return false;
        if (end == NULL) break;
        start = end + 1;
    }
    return true;
}

bool omt_valid_host(const char *value) {
    const char *start;
    if (value == NULL || *value == '\0' || strlen(value) > 253U || contains_control(value)) return false;
    start = value;
    for (;;) {
        const char *end = strchr(start, '.');
        const size_t count = end == NULL ? strlen(start) : (size_t)(end - start);
        char label[64];
        if (count == 0U || count > 63U || start[0] == '-' || start[count - 1U] == '-') return false;
        memcpy(label, start, count); label[count] = '\0';
        if (!ascii_token(label, "-")) return false;
        if (end == NULL) break;
        start = end + 1;
    }
    return true;
}

bool omt_valid_username(const char *value) {
    return value != NULL && strlen(value) <= 64U && !contains_control(value) &&
           ascii_token(value, "._-");
}

bool omt_valid_remote_directory(const char *value) {
    const size_t length = value == NULL ? 0U : strlen(value);
    return length >= 2U && length <= 240U && value[0] == '/' && value[length - 1U] != '/' &&
           strstr(value, "//") == NULL && valid_manifest_name(value + 1);
}

bool omt_connection_validate(const omt_connection *c, char *error, size_t size) {
    struct stat status;
    if (c == NULL || !omt_valid_host(c->host)) {
        set_error(error, size, "Pi host must be a valid IPv4 address or DNS host name."); return false;
    }
    if (!omt_valid_username(c->username)) {
        set_error(error, size, "SSH username contains invalid characters."); return false;
    }
    if (c->port == 0U) { set_error(error, size, "SSH port must be between 1 and 65535."); return false; }
    if (c->auth == OMT_AUTH_PASSWORD && (c->password == NULL || *c->password == '\0')) {
        set_error(error, size, "SSH password is required for password authentication."); return false;
    }
    if (c->auth == OMT_AUTH_KEY &&
        (c->key_path == NULL || strlen(c->key_path) > 4096U || stat(c->key_path, &status) != 0 ||
         !S_ISREG(status.st_mode))) {
        set_error(error, size, "SSH private-key file does not exist."); return false;
    }
    const char *secrets[] = {c->password, c->key_passphrase, c->sudo_password};
    for (size_t i = 0U; i < 3U; ++i) {
        if (secrets[i] != NULL && (strlen(secrets[i]) > 4096U || contains_control(secrets[i]))) {
            set_error(error, size, "Authentication secrets are invalid or exceed 4096 bytes."); return false;
        }
    }
    return true;
}

bool omt_options_validate(const omt_deploy_options *o, bool require_project, char *error, size_t size) {
    struct stat status;
    if (o == NULL || (require_project && (o->project_root == NULL ||
        stat(o->project_root, &status) != 0 || !S_ISDIR(status.st_mode)))) {
        set_error(error, size, "Project root does not exist."); return false;
    }
    if (!omt_valid_remote_directory(o->remote_directory)) {
        set_error(error, size, "Remote install directory is not a normalized safe absolute path."); return false;
    }
    if (!ascii_token(o->image_name, "._-:") || !ascii_token(o->tarball_name, "._-")) {
        set_error(error, size, "Image and archive names contain unsafe characters."); return false;
    }
    return true;
}

bool omt_wifi_validate(const omt_wifi_settings *w, char *error, size_t size) {
    size_t length;
    bool hex = true;
    if (w == NULL || w->ssid == NULL || *w->ssid == '\0' || strlen(w->ssid) > 32U ||
        contains_control(w->ssid) || !valid_utf8(w->ssid)) {
        set_error(error, size, "Wi-Fi SSID must contain 1-32 UTF-8 bytes and no control characters."); return false;
    }
    if (w->password == NULL || contains_control(w->password)) {
        set_error(error, size, "Wi-Fi password must not contain control characters."); return false;
    }
    length = strlen(w->password);
    for (size_t i = 0U; i < length; ++i) {
        const unsigned char c = (unsigned char)w->password[i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'))) hex = false;
        if (c < 0x20U || c > 0x7eU) {
            set_error(error, size, "Wi-Fi password contains non-printable characters."); return false;
        }
    }
    if (!((length >= 8U && length <= 63U) || (length == 64U && hex))) {
        set_error(error, size, "Wi-Fi password must be 8-63 printable ASCII characters or a 64-digit hex PSK.");
        return false;
    }
    return true;
}

char *omt_shell_quote(const char *value) {
    size_t length = 3U;
    char *result;
    char *out;
    if (value == NULL) value = "";
    for (const char *p = value; *p != '\0'; ++p) length += *p == '\'' ? 4U : 1U;
    result = malloc(length);
    if (result == NULL) return NULL;
    out = result; *out++ = '\'';
    for (const char *p = value; *p != '\0'; ++p) {
        if (*p == '\'') { memcpy(out, "'\\''", 4U); out += 4U; } else { *out++ = *p; }
    }
    *out++ = '\''; *out = '\0';
    return result;
}

static bool regular_marker(const char *directory) {
    char path[4096];
    struct stat status;
    if (snprintf(path, sizeof(path), "%s/deploy/manifest-v3.txt", directory) < 0) return false;
    return stat(path, &status) == 0 && S_ISREG(status.st_mode);
}

static bool safe_regular_file(const char *path, struct stat *status) {
#ifdef _WIN32
    const DWORD attributes = GetFileAttributesA(path);
    return attributes != INVALID_FILE_ATTRIBUTES &&
           (attributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)) == 0U &&
           stat(path, status) == 0 && S_ISREG(status->st_mode);
#else
    return lstat(path, status) == 0 && S_ISREG(status->st_mode) && !S_ISLNK(status->st_mode);
#endif
}

static bool is_path_separator(char value) {
    return value == '/' || value == '\\';
}

static char *last_path_separator(char *path) {
    char *separator = NULL;
    for (char *cursor = path; *cursor != '\0'; ++cursor) {
        if (is_path_separator(*cursor)) separator = cursor;
    }
    return separator;
}

char *omt_discover_project_root(const char *executable, const char *working) {
    const char *starts[2] = {working, executable};
    for (size_t i = 0U; i < 2U; ++i) {
        char current[4096];
        size_t length;
        if (starts[i] == NULL || *starts[i] == '\0' || strlen(starts[i]) >= sizeof(current)) continue;
        strcpy(current, starts[i]);
        length = strlen(current);
        while (length > 1U && is_path_separator(current[length - 1U])) {
#ifdef _WIN32
            /* Preserve a drive root such as C:\ so ascent can terminate cleanly. */
            if (length == 3U && current[1] == ':') break;
#endif
            current[--length] = '\0';
        }
        for (unsigned level = 0U; level <= 8U; ++level) {
            char *separator;
            if (regular_marker(current)) return strdup(current);
            separator = last_path_separator(current);
            if (separator == NULL || separator == current) break;
#ifdef _WIN32
            if (separator == current + 2 && current[1] == ':') {
                if (separator[1] == '\0') break;
                separator[1] = '\0';
                continue;
            }
#endif
            *separator = '\0';
        }
    }
    return strdup(working != NULL && *working != '\0' ? working :
                  (executable != NULL ? executable : ""));
}

bool omt_load_manifest(const char *path, omt_string_list *list, char *error, size_t size) {
    struct stat status;
    FILE *input;
    char line[512];
    bool transaction = false;
    bool manifest = false;
    if (list == NULL) return false;
    list->items = NULL; list->count = 0U;
    if (path == NULL || !safe_regular_file(path, &status) || status.st_size > 32768) {
        set_error(error, size, "Deployment manifest is missing, unsafe, or oversized."); return false;
    }
    input = fopen(path, "rb");
    if (input == NULL || fgets(line, sizeof(line), input) == NULL || strcmp(line, "version=3\n") != 0) {
        if (input != NULL) fclose(input);
        set_error(error, size, "Deployment manifest must begin with version=3."); return false;
    }
    while (fgets(line, sizeof(line), input) != NULL) {
        const size_t n = strlen(line);
        char **grown;
        if (n == 0U || line[n - 1U] != '\n' || list->count >= 128U) goto invalid;
        line[n - 1U] = '\0';
        if (!valid_manifest_name(line)) goto invalid;
        for (size_t i = 0U; i < list->count; ++i) if (strcmp(list->items[i], line) == 0) goto invalid;
        grown = realloc(list->items, (list->count + 1U) * sizeof(*grown));
        if (grown == NULL) goto invalid;
        list->items = grown;
        list->items[list->count] = strdup(line);
        if (list->items[list->count] == NULL) goto invalid;
        transaction |= strcmp(line, "deploy/transaction.sh") == 0;
        manifest |= strcmp(line, "deploy/manifest-v3.txt") == 0;
        ++list->count;
    }
    if (ferror(input) != 0 || !transaction || !manifest) goto invalid;
    fclose(input); return true;
invalid:
    fclose(input); omt_string_list_free(list);
    set_error(error, size, "Deployment manifest contains an invalid, duplicate, or incomplete path.");
    return false;
}

void omt_string_list_free(omt_string_list *list) {
    if (list == NULL) return;
    for (size_t i = 0U; i < list->count; ++i) free(list->items[i]);
    free(list->items); list->items = NULL; list->count = 0U;
}

bool omt_random_token(size_t count, char *output, size_t output_size) {
    unsigned char random[64];
    static const char hex[] = "0123456789abcdef";
    if (count == 0U || count > sizeof(random) || output == NULL || output_size < count * 2U + 1U) return false;
#ifdef _WIN32
    if (BCryptGenRandom(NULL, random, (ULONG)count, BCRYPT_USE_SYSTEM_PREFERRED_RNG) != 0) return false;
#else
    size_t offset = 0U;
    while (offset < count) {
        const ssize_t got = getrandom(random + offset, count - offset, 0);
        if (got < 0) { if (errno == EINTR) continue; return false; }
        offset += (size_t)got;
    }
#endif
    for (size_t i = 0U; i < count; ++i) {
        output[i * 2U] = hex[random[i] >> 4U];
        output[i * 2U + 1U] = hex[random[i] & 0x0fU];
    }
    output[count * 2U] = '\0';
    omt_secure_clear((char *)random, sizeof(random));
    return true;
}

void omt_secure_clear(char *value, size_t capacity) {
    volatile unsigned char *p = (volatile unsigned char *)value;
    if (value == NULL) return;
    while (capacity-- > 0U) *p++ = 0U;
}
