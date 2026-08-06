#define _POSIX_C_SOURCE 200809L
#include "deployer.h"

#include <libssh2.h>
#include <libssh2_sftp.h>

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
typedef SOCKET omt_socket;
#define OMT_INVALID_SOCKET INVALID_SOCKET
#define omt_close_socket closesocket
#else
#include <fcntl.h>
#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>
typedef int omt_socket;
#define OMT_INVALID_SOCKET (-1)
#define omt_close_socket close
#endif

struct omt_ssh_client {
    omt_socket socket;
    LIBSSH2_SESSION *session;
};

static uint64_t monotonic_milliseconds(void) {
#ifdef _WIN32
    return (uint64_t)GetTickCount64();
#else
    struct timespec value;
    if (clock_gettime(CLOCK_MONOTONIC, &value) != 0) {
        return 0U;
    }
    return (uint64_t)value.tv_sec * 1000U + (uint64_t)value.tv_nsec / 1000000U;
#endif
}

static void set_error(char *error, size_t size, const char *message) {
    if (error != NULL && size > 0U) (void)snprintf(error,size,"%s",message);
}

static void session_error(LIBSSH2_SESSION *session, const char *prefix, char *error, size_t size) {
    char *message=NULL; int length=0;
    (void)libssh2_session_last_error(session,&message,&length,0);
    if(error!=NULL&&size>0U)(void)snprintf(error,size,"%s%s%.*s",prefix,
        message==NULL?"":": ",message==NULL?0:length,message==NULL?"":message);
}

static bool socket_pending(void) {
#ifdef _WIN32
    const int e=WSAGetLastError();return e==WSAEINPROGRESS||e==WSAEWOULDBLOCK||e==WSAEINVAL;
#else
    return errno==EINPROGRESS||errno==EWOULDBLOCK;
#endif
}

static bool connect_timeout(omt_socket socket, const struct addrinfo *address) {
#ifdef _WIN32
    u_long enabled=1;
    if(ioctlsocket(socket,(long)FIONBIO,&enabled)!=0)return false;
    if(connect(socket,address->ai_addr,(int)address->ai_addrlen)==0)goto restore_success;
#else
    int original=0;
    original=fcntl(socket,F_GETFL,0);
    if(original<0||fcntl(socket,F_SETFL,original|O_NONBLOCK)!=0)return false;
    if(connect(socket,address->ai_addr,address->ai_addrlen)==0)goto restore_success;
#endif
    if(!socket_pending())goto restore_failure;
    {
        fd_set writes,errors;struct timeval timeout={15,0};int ready,socket_error=0;
        FD_ZERO(&writes);FD_ZERO(&errors);FD_SET(socket,&writes);FD_SET(socket,&errors);
#ifdef _WIN32
        ready=select(0,NULL,&writes,&errors,&timeout);
        {int length=(int)sizeof(socket_error);if(ready<=0||getsockopt(socket,SOL_SOCKET,SO_ERROR,(char *)&socket_error,&length)!=0||socket_error!=0)goto restore_failure;}
#else
        ready=select(socket+1,NULL,&writes,&errors,&timeout);
        {socklen_t length=sizeof(socket_error);if(ready<=0||getsockopt(socket,SOL_SOCKET,SO_ERROR,&socket_error,&length)!=0||socket_error!=0)goto restore_failure;}
#endif
    }
restore_success:
#ifdef _WIN32
    {u_long disabled=0;return ioctlsocket(socket,(long)FIONBIO,&disabled)==0;}
#else
    return fcntl(socket,F_SETFL,original)==0;
#endif
restore_failure:
#ifdef _WIN32
    {u_long disabled=0;(void)ioctlsocket(socket,(long)FIONBIO,&disabled);}
#else
    (void)fcntl(socket,F_SETFL,original);
#endif
    return false;
}

static int key_mask(int type) {
    switch(type){
        case LIBSSH2_HOSTKEY_TYPE_RSA:return LIBSSH2_KNOWNHOST_KEY_SSHRSA;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_256:return LIBSSH2_KNOWNHOST_KEY_ECDSA_256;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_384:return LIBSSH2_KNOWNHOST_KEY_ECDSA_384;
        case LIBSSH2_HOSTKEY_TYPE_ECDSA_521:return LIBSSH2_KNOWNHOST_KEY_ECDSA_521;
        case LIBSSH2_HOSTKEY_TYPE_ED25519:return LIBSSH2_KNOWNHOST_KEY_ED25519;
        default:return LIBSSH2_KNOWNHOST_KEY_UNKNOWN;
    }
}

static bool verify_host(omt_ssh_client *client, const omt_connection *connection,
                        char *error, size_t error_size) {
#ifdef _WIN32
    const char *home=getenv("USERPROFILE");const char *separator="\\";
#else
    const char *home=getenv("HOME");const char *separator="/";
#endif
    char path[4096];FILE *probe;LIBSSH2_KNOWNHOSTS *hosts;size_t key_length=0U;int key_type=0;
    const char *key;struct libssh2_knownhost *matched=NULL;int result;
    if(home==NULL||*home=='\0'||snprintf(path,sizeof(path),"%s%s.ssh%sknown_hosts",home,separator,separator)<0){
        set_error(error,error_size,"Home directory is unavailable for strict host-key verification.");return false;
    }
    probe=fopen(path,"rb");if(probe==NULL){set_error(error,error_size,
        "Strict host-key verification requires ~/.ssh/known_hosts. Add the Pi key first.");return false;}fclose(probe);
    hosts=libssh2_knownhost_init(client->session);if(hosts==NULL){set_error(error,error_size,"Unable to initialize SSH known-host verification.");return false;}
    if(libssh2_knownhost_readfile(hosts,path,LIBSSH2_KNOWNHOST_FILE_OPENSSH)<0){
        libssh2_knownhost_free(hosts);set_error(error,error_size,"Unable to read ~/.ssh/known_hosts.");return false;
    }
    key=libssh2_session_hostkey(client->session,&key_length,&key_type);
    result=key==NULL?LIBSSH2_KNOWNHOST_CHECK_FAILURE:
        libssh2_knownhost_checkp(hosts,connection->host,connection->port,key,key_length,
            LIBSSH2_KNOWNHOST_TYPE_PLAIN|LIBSSH2_KNOWNHOST_KEYENC_RAW|key_mask(key_type),&matched);
    libssh2_knownhost_free(hosts);
    if(result!=LIBSSH2_KNOWNHOST_CHECK_MATCH){set_error(error,error_size,
        "The SSH host key is unknown or changed; strict verification refused the connection.");return false;}
    return true;
}

omt_ssh_client *omt_ssh_connect(const omt_connection *connection, char *error, size_t error_size) {
    static bool initialized=false;
    struct addrinfo hints,*addresses=NULL,*address;char service[6];
    omt_ssh_client *client=NULL;
#ifdef _WIN32
    static bool winsock=false;if(!winsock){WSADATA data;if(WSAStartup(MAKEWORD(2,2),&data)!=0){set_error(error,error_size,"Unable to initialize WinSock.");return NULL;}winsock=true;}
#endif
    if(!initialized){if(libssh2_init(0)!=0){set_error(error,error_size,"Unable to initialize libssh2.");return NULL;}initialized=true;}
    client=calloc(1U,sizeof(*client));if(client==NULL)return NULL;client->socket=OMT_INVALID_SOCKET;
    memset(&hints,0,sizeof(hints));hints.ai_family=AF_UNSPEC;hints.ai_socktype=SOCK_STREAM;
    (void)snprintf(service,sizeof(service),"%u",(unsigned)connection->port);
    if(getaddrinfo(connection->host,service,&hints,&addresses)!=0){set_error(error,error_size,"Unable to resolve the SSH host.");goto fail;}
    for(address=addresses;address!=NULL;address=address->ai_next){
        client->socket=socket(address->ai_family,address->ai_socktype,address->ai_protocol);
        if(client->socket!=OMT_INVALID_SOCKET&&connect_timeout(client->socket,address))break;
        if(client->socket!=OMT_INVALID_SOCKET)omt_close_socket(client->socket);
        client->socket=OMT_INVALID_SOCKET;
    }
    freeaddrinfo(addresses);addresses=NULL;
    if(client->socket==OMT_INVALID_SOCKET){set_error(error,error_size,"Unable to connect to the SSH host.");goto fail;}
    client->session=libssh2_session_init();if(client->session==NULL){set_error(error,error_size,"Unable to allocate an SSH session.");goto fail;}
    libssh2_session_set_blocking(client->session,1);libssh2_session_set_timeout(client->session,60000L);
    if(libssh2_session_handshake(client->session,client->socket)!=0){session_error(client->session,"SSH handshake failed",error,error_size);goto fail;}
    if(!verify_host(client,connection,error,error_size))goto fail;
    if(connection->auth==OMT_AUTH_PASSWORD){
        if(libssh2_userauth_password_ex(client->session,connection->username,(unsigned int)strlen(connection->username),
            connection->password,(unsigned int)strlen(connection->password),NULL)!=0)goto auth_fail;
    }else if(libssh2_userauth_publickey_fromfile_ex(client->session,connection->username,
        (unsigned int)strlen(connection->username),NULL,connection->key_path,
        connection->key_passphrase==NULL||*connection->key_passphrase=='\0'?NULL:connection->key_passphrase)!=0){
auth_fail:session_error(client->session,"SSH authentication failed",error,error_size);goto fail;
    }
    return client;
fail:
    if(addresses!=NULL)freeaddrinfo(addresses);
    omt_ssh_close(client);return NULL;
}

void omt_ssh_close(omt_ssh_client *client) {
    if(client==NULL)return;
    if(client->session!=NULL){(void)libssh2_session_disconnect(client->session,"Raspberry Pi OMT deployer closed the session");libssh2_session_free(client->session);}
    if(client->socket!=OMT_INVALID_SOCKET)omt_close_socket(client->socket);
    free(client);
}

static bool append_output(char **target,size_t *used,size_t other,const char *bytes,size_t count,
                          omt_text_callback progress,void *context,char *error,size_t error_size){
    char *grown;if(*used+other+count>OMT_DEPLOYER_OUTPUT_LIMIT){set_error(error,error_size,"Remote command output exceeded 4 MiB.");return false;}
    grown=realloc(*target,*used+count+1U);
    if(grown==NULL){set_error(error,error_size,"Unable to allocate remote command output.");return false;}
    *target=grown;
    memcpy(grown+*used,bytes,count);*used+=count;grown[*used]='\0';if(progress!=NULL)progress(bytes,count,context);return true;
}

void omt_remote_result_free(omt_remote_result *result){
    if(result!=NULL){free(result->output);free(result->error_output);memset(result,0,sizeof(*result));result->exit_code=-1;}
}

bool omt_ssh_run(omt_ssh_client *client,const char *command,const char *input,
                 omt_text_callback progress,omt_stop_callback stop,void *context,
                 omt_remote_result *result,char *error,size_t error_size){
    LIBSSH2_CHANNEL *channel;size_t written=0U,out_used=0U,err_used=0U;uint64_t deadline;
    char buffer[16384];bool ok=true;
    if(result==NULL)return false;
    memset(result,0,sizeof(*result));result->exit_code=-1;
    channel=libssh2_channel_open_session(client->session);if(channel==NULL){session_error(client->session,"Unable to open remote command channel",error,error_size);return false;}
    if(libssh2_channel_process_startup(channel,"exec",4,command,(unsigned int)strlen(command))!=0){session_error(client->session,"Unable to start remote command",error,error_size);ok=false;goto done;}
    if(input==NULL)input="";
    while(written<strlen(input)){const ssize_t count=libssh2_channel_write(channel,input+written,strlen(input)-written);if(count<=0){session_error(client->session,"Unable to write remote command input",error,error_size);ok=false;goto done;}written+=(size_t)count;}
    (void)libssh2_channel_send_eof(channel);libssh2_session_set_blocking(client->session,0);
    deadline=monotonic_milliseconds()+60000U;
    while(libssh2_channel_eof(channel)==0){
        bool received=false;ssize_t count;
        if(stop!=NULL&&stop(context)){set_error(error,error_size,"Operation cancelled.");ok=false;goto blocking_done;}
        count=libssh2_channel_read(channel,buffer,sizeof(buffer));
        if(count>0){if(!append_output(&result->output,&out_used,err_used,buffer,(size_t)count,progress,context,error,error_size)){ok=false;goto blocking_done;}received=true;}
        else if(count<0&&count!=LIBSSH2_ERROR_EAGAIN){session_error(client->session,"Unable to read remote output",error,error_size);ok=false;goto blocking_done;}
        count=libssh2_channel_read_stderr(channel,buffer,sizeof(buffer));
        if(count>0){if(!append_output(&result->error_output,&err_used,out_used,buffer,(size_t)count,progress,context,error,error_size)){ok=false;goto blocking_done;}received=true;}
        else if(count<0&&count!=LIBSSH2_ERROR_EAGAIN){session_error(client->session,"Unable to read remote error output",error,error_size);ok=false;goto blocking_done;}
        if(received)deadline=monotonic_milliseconds()+60000U;
        else{
            fd_set reads,writes;struct timeval timeout={1,0};const int directions=libssh2_session_block_directions(client->session);
            if(monotonic_milliseconds()>=deadline){set_error(error,error_size,"Remote command produced no output for 60 seconds.");ok=false;goto blocking_done;}
            FD_ZERO(&reads);FD_ZERO(&writes);if((directions&LIBSSH2_SESSION_BLOCK_INBOUND)!=0||directions==0)FD_SET(client->socket,&reads);
            if((directions&LIBSSH2_SESSION_BLOCK_OUTBOUND)!=0)FD_SET(client->socket,&writes);
#ifdef _WIN32
            (void)select(0,&reads,&writes,NULL,&timeout);
#else
            (void)select(client->socket+1,&reads,&writes,NULL,&timeout);
#endif
        }
    }
    result->exit_code=libssh2_channel_get_exit_status(channel);
blocking_done:libssh2_session_set_blocking(client->session,1);
done:(void)libssh2_channel_close(channel);libssh2_channel_free(channel);
    if(result->output==NULL)result->output=strdup("");
    if(result->error_output==NULL)result->error_output=strdup("");
    return ok;
}

bool omt_ssh_upload(omt_ssh_client *client,const char *local_path,const char *remote_path,
                    omt_upload_callback progress,omt_stop_callback stop,void *context,
                    char *error,size_t error_size){
    LIBSSH2_SFTP *sftp=NULL;LIBSSH2_SFTP_HANDLE *output=NULL;FILE *input=NULL;
    char buffer[65536];uint64_t uploaded=0U,total=0U;bool ok=false;
    input=fopen(local_path,"rb");if(input==NULL){set_error(error,error_size,"Unable to open local deployment artifact.");goto done;}
    if(fseek(input,0,SEEK_END)!=0)goto done;
    {const long length=ftell(input);if(length<0)goto done;total=(uint64_t)length;}
    rewind(input);
    sftp=libssh2_sftp_init(client->session);if(sftp==NULL){session_error(client->session,"Unable to initialize SFTP",error,error_size);goto done;}
    output=libssh2_sftp_open_ex(sftp,remote_path,(unsigned int)strlen(remote_path),
        LIBSSH2_FXF_WRITE|LIBSSH2_FXF_CREAT|LIBSSH2_FXF_TRUNC,
        LIBSSH2_SFTP_S_IRUSR|LIBSSH2_SFTP_S_IWUSR,LIBSSH2_SFTP_OPENFILE);
    if(output==NULL){session_error(client->session,"Unable to create remote deployment artifact",error,error_size);goto done;}
    for(;;){const size_t available=fread(buffer,1U,sizeof(buffer),input);size_t offset=0U;
        while(offset<available){ssize_t count;if(stop!=NULL&&stop(context)){set_error(error,error_size,"Operation cancelled.");goto done;}
            count=libssh2_sftp_write(output,buffer+offset,available-offset);if(count<=0){session_error(client->session,"SFTP upload failed",error,error_size);goto done;}
            offset+=(size_t)count;uploaded+=(uint64_t)count;if(progress!=NULL)progress(uploaded,total,context);}
        if(available<sizeof(buffer)){if(ferror(input)!=0)set_error(error,error_size,"Unable to read local deployment artifact.");else ok=true;break;}
    }
done:if(output!=NULL)libssh2_sftp_close(output);if(sftp!=NULL)libssh2_sftp_shutdown(sftp);if(input!=NULL)fclose(input);return ok;
}
