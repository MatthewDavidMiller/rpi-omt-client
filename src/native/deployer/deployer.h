#ifndef OMT_DEPLOYER_H
#define OMT_DEPLOYER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OMT_DEPLOYER_ERROR_SIZE 1024U
#define OMT_DEPLOYER_OUTPUT_LIMIT (4U * 1024U * 1024U)

typedef enum {
    OMT_AUTH_PASSWORD = 0,
    OMT_AUTH_KEY = 1
} omt_auth_method;

typedef struct {
    char *host;
    char *username;
    uint16_t port;
    omt_auth_method auth;
    char *password;
    char *key_path;
    char *key_passphrase;
    char *sudo_password;
} omt_connection;

typedef struct {
    char *project_root;
    char *remote_directory;
    char *image_name;
    char *tarball_name;
    bool build_image;
} omt_deploy_options;

typedef struct {
    char *ssid;
    char *password;
    bool connect;
} omt_wifi_settings;

typedef struct {
    char **items;
    size_t count;
} omt_string_list;

typedef struct {
    int exit_code;
    char *output;
} omt_process_result;

typedef struct {
    int exit_code;
    char *output;
    char *error_output;
} omt_remote_result;

typedef struct {
    const char *name;
    const char *text;
    size_t text_size;
} omt_legal_document;

typedef void (*omt_text_callback)(const char *text, size_t size, void *context);
typedef bool (*omt_stop_callback)(void *context);
typedef void (*omt_upload_callback)(uint64_t uploaded, uint64_t total, void *context);

bool omt_valid_host(const char *value);
bool omt_valid_username(const char *value);
bool omt_valid_remote_directory(const char *value);
bool omt_connection_validate(const omt_connection *connection, char *error, size_t error_size);
bool omt_options_validate(const omt_deploy_options *options, bool require_project,
                          char *error, size_t error_size);
bool omt_wifi_validate(const omt_wifi_settings *settings, char *error, size_t error_size);
char *omt_shell_quote(const char *value);
char *omt_discover_project_root(const char *executable_directory, const char *working_directory);
bool omt_load_manifest(const char *path, omt_string_list *list, char *error, size_t error_size);
bool omt_random_token(size_t byte_count, char *output, size_t output_size);
void omt_secure_clear(char *value, size_t capacity);
void omt_string_list_free(omt_string_list *list);

bool omt_sha256_file(const char *path, char output[65], char *error, size_t error_size);
bool omt_run_process(const char *const *arguments, const char *working_directory,
                     omt_text_callback progress, omt_stop_callback stop, void *context,
                     omt_process_result *result, char *error, size_t error_size);
void omt_process_result_free(omt_process_result *result);

size_t omt_legal_documents(const omt_legal_document **documents);

typedef struct omt_ssh_client omt_ssh_client;
omt_ssh_client *omt_ssh_connect(const omt_connection *connection, char *error, size_t error_size);
void omt_ssh_close(omt_ssh_client *client);
bool omt_ssh_run(omt_ssh_client *client, const char *command, const char *input,
                 omt_text_callback progress, omt_stop_callback stop, void *context,
                 omt_remote_result *result,
                 char *error, size_t error_size);
bool omt_ssh_upload(omt_ssh_client *client, const char *local_path, const char *remote_path,
                    omt_upload_callback progress, omt_stop_callback stop, void *context,
                    char *error, size_t error_size);
void omt_remote_result_free(omt_remote_result *result);

typedef struct omt_deployment_service omt_deployment_service;
omt_deployment_service *omt_deployment_service_create(const char *version,
    omt_text_callback event, omt_stop_callback stop, void *context);
void omt_deployment_service_destroy(omt_deployment_service *service);
bool omt_install_prerequisites(omt_deployment_service *service, const char *project_root,
                               char *error, size_t error_size);
bool omt_test_connection(omt_deployment_service *service, const omt_connection *connection,
                         char *error, size_t error_size);
bool omt_deploy(omt_deployment_service *service, const omt_connection *connection,
                const omt_deploy_options *options, char *error, size_t error_size);
bool omt_manage(omt_deployment_service *service, const omt_connection *connection,
                const char *remote_directory, const char *action, char **output,
                char *error, size_t error_size);
bool omt_apply_wifi(omt_deployment_service *service, const omt_connection *connection,
                    const omt_wifi_settings *settings, char *error, size_t error_size);

#ifdef __cplusplus
}
#endif
#endif
