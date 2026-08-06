#define _POSIX_C_SOURCE 200809L
#include "deployer.h"

#include <ctype.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <unistd.h>
#endif

struct omt_deployment_service {
    char *version;
    omt_text_callback event;
    omt_stop_callback stop;
    void *context;
    char *secrets[4];
};

typedef struct {
    off_t size;
    time_t modified;
    char digest[65];
} artifact_identity;

static const char *arm_check_image="debian:bookworm-slim@sha256:4724b8cc51e33e398f0e2e15e18d5ec2851ff0c2280647e1310bc1642182655d";
static const char *binfmt_image="tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0";
static const char *platform_probe="uname -m && . /etc/os-release && printf '%s\\n' \"$ID\" && cat /etc/alpine-release && tr -d '\\000' < /proc/device-tree/model && printf '\\n'";

#if defined(__GNUC__) || defined(__clang__)
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wformat-nonliteral"
#endif
static char *format(const char *pattern,...) {
    va_list args,copy;int needed;char *result;
    va_start(args,pattern);va_copy(copy,args);needed=vsnprintf(NULL,0,pattern,copy);va_end(copy);
    if(needed<0){va_end(args);return NULL;}result=malloc((size_t)needed+1U);
    if(result!=NULL)(void)vsnprintf(result,(size_t)needed+1U,pattern,args);
    va_end(args);return result;
}
#if defined(__GNUC__) || defined(__clang__)
#pragma GCC diagnostic pop
#endif

static char *join(const char *left,const char *right) {
    return format("%s%s%s",left,left[0]!='\0'&&left[strlen(left)-1U]=='/'?"":"/",right);
}

static void service_secrets(omt_deployment_service *service,const omt_connection *c,const char *extra) {
    const char *values[4]={c->password,c->key_passphrase,c->sudo_password,extra};
    for(size_t i=0U;i<4U;++i){
        if(service->secrets[i]!=NULL){omt_secure_clear(service->secrets[i],strlen(service->secrets[i]));free(service->secrets[i]);}
        service->secrets[i]=values[i]!=NULL?strdup(values[i]):NULL;
    }
}

static char *redact(const omt_deployment_service *service,const char *message) {
    char *safe=strdup(message==NULL?"":message);if(safe==NULL)return NULL;
    for(size_t i=0U;i<4U;++i){const char *secret=service->secrets[i];size_t length,offset=0U;
        if(secret==NULL||*secret=='\0')continue;
        length=strlen(secret);
        /* Resume past the marker just written: a secret that is a substring of
           "[redacted]" would otherwise match inside it and never terminate. */
        for(;;){char *at=strstr(safe+offset,secret);char *next;size_t before,total;
            if(at==NULL)break;
            before=(size_t)(at-safe);total=strlen(safe)-length+10U;
            next=malloc(total+1U);if(next==NULL)break;memcpy(next,safe,before);memcpy(next+before,"[redacted]",10U);
            strcpy(next+before+10U,at+length);free(safe);safe=next;offset=before+10U;}}
    return safe;
}

static void emit(omt_deployment_service *service,const char *text,size_t size) {
    char *copy,*safe;if(service->event==NULL)return;copy=malloc(size+1U);if(copy==NULL)return;
    memcpy(copy,text,size);copy[size]='\0';safe=redact(service,copy);free(copy);
    if(safe!=NULL){service->event(safe,strlen(safe),service->context);free(safe);}
}
#define EMIT_LITERAL(service, text) emit((service),(text),sizeof(text)-1U)

static void progress_event(const char *text,size_t size,void *context) {
    emit((omt_deployment_service *)context,text,size);
}

static bool service_stop(void *context) {
    omt_deployment_service *service=context;
    return service->stop!=NULL&&service->stop(service->context);
}

static bool checkpoint(omt_deployment_service *service,char *error,size_t error_size) {
    if(service_stop(service)){if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Operation cancelled.");return false;}return true;
}

static bool process_success(omt_deployment_service *service,const char *const *arguments,
                            const char *directory,const char *operation,char *error,size_t error_size) {
    omt_process_result result;char detail[OMT_DEPLOYER_ERROR_SIZE]={0};bool ran,ok;
    ran=omt_run_process(arguments,directory,progress_event,service_stop,service,&result,detail,sizeof(detail));
    ok=ran&&result.exit_code==0;
    if(!ok&&error!=NULL&&error_size>0U){char *message=format("%s failed:\n%s%s",operation,detail,result.output==NULL?"":result.output);
        char *safe=redact(service,message==NULL?"":message);(void)snprintf(error,error_size,"%s",safe==NULL?"Operation failed.":safe);free(safe);free(message);}
    omt_process_result_free(&result);return ok;
}

static bool remote_success(omt_deployment_service *service,omt_ssh_client *ssh,const char *command,
                           const char *input,const char *operation,omt_remote_result *out,
                           char *error,size_t error_size) {
    omt_remote_result local;char detail[OMT_DEPLOYER_ERROR_SIZE]={0};bool ran,ok;
    ran=omt_ssh_run(ssh,command,input,progress_event,service_stop,service,&local,detail,sizeof(detail));
    ok=ran&&local.exit_code==0;
    if(!ok&&error!=NULL&&error_size>0U){char *message=format("%s failed:\n%s%s%s",operation,detail,
        local.error_output==NULL?"":local.error_output,local.output==NULL?"":local.output);
        char *safe=redact(service,message==NULL?"":message);(void)snprintf(error,error_size,"%s",safe==NULL?"Remote operation failed.":safe);free(safe);free(message);}
    if(out!=NULL)*out=local;else omt_remote_result_free(&local);return ok;
}

static bool platform(omt_deployment_service *service,omt_ssh_client *ssh,char *error,size_t error_size) {
    omt_remote_result result;bool ok=remote_success(service,ssh,platform_probe,"","Remote platform probe",&result,error,error_size);
    if(ok){char architecture[64]={0},system[64]={0},release[64]={0},model[128]={0};
        if(sscanf(result.output,"%63[^\n]\n%63[^\n]\n%63[^\n]\n%127[^\n]",architecture,system,release,model)!=4||
           strcmp(architecture,"aarch64")!=0||strcmp(system,"alpine")!=0||strncmp(release,"3.23.",5U)!=0||
           strncmp(model,"Raspberry Pi 5",14U)!=0){
            (void)snprintf(error,error_size,"The target must be a Raspberry Pi 5 running Alpine Linux 3.23 aarch64.");ok=false;}}
    omt_remote_result_free(&result);return ok;
}

omt_deployment_service *omt_deployment_service_create(const char *version,omt_text_callback event,
                                                        omt_stop_callback stop,void *context) {
    omt_deployment_service *service=calloc(1U,sizeof(*service));if(service==NULL)return NULL;
    service->version=strdup(version==NULL?"unknown":version);service->event=event;service->stop=stop;service->context=context;
    if(service->version==NULL){free(service);return NULL;}return service;
}

void omt_deployment_service_destroy(omt_deployment_service *service) {
    if(service==NULL)return;
    for(size_t i=0U;i<4U;++i)if(service->secrets[i]!=NULL){omt_secure_clear(service->secrets[i],strlen(service->secrets[i]));free(service->secrets[i]);}
    free(service->version);free(service);
}

bool omt_install_prerequisites(omt_deployment_service *service,const char *root,char *error,size_t error_size) {
    const char *install[]={"docker","run","--privileged","--rm",binfmt_image,"--install","arm64",NULL};
    const char *verify[]={"docker","run","--rm","--platform","linux/arm64","--entrypoint","/bin/sh",
                          arm_check_image,"-c","test \"$(uname -m)\" = aarch64",NULL};
    if(!checkpoint(service,error,error_size))return false;
    EMIT_LITERAL(service,"Installing pinned ARM64 emulation support...\n");
    return process_success(service,install,root,"ARM64 emulator installation",error,error_size)&&
           process_success(service,verify,root,"ARM64 emulator verification",error,error_size)&&checkpoint(service,error,error_size);
}

bool omt_test_connection(omt_deployment_service *service,const omt_connection *connection,char *error,size_t error_size) {
    omt_ssh_client *ssh;if(!checkpoint(service,error,error_size)||!omt_connection_validate(connection,error,error_size))return false;
    service_secrets(service,connection,NULL);EMIT_LITERAL(service,"Testing strict SSH connection...\n");
    ssh=omt_ssh_connect(connection,error,error_size);if(ssh==NULL)return false;
    bool ok=platform(service,ssh,error,error_size);omt_ssh_close(ssh);
    if(ok){EMIT_LITERAL(service,"SSH connection and platform checks succeeded.\n");ok=checkpoint(service,error,error_size);}return ok;
}

static char *sudo_prefix(const omt_connection *c) {
    return strdup((c->sudo_password!=NULL&&*c->sudo_password!='\0')||
                  (c->auth==OMT_AUTH_PASSWORD&&c->password!=NULL&&*c->password!='\0')?"sudo -S -p ''":"sudo -n");
}

static char *sudo_input(const omt_connection *c) {
    const char *password=c->sudo_password!=NULL&&*c->sudo_password!='\0'?c->sudo_password:
        (c->auth==OMT_AUTH_PASSWORD?c->password:"");return format("%s%s",password==NULL?"":password,
        password!=NULL&&*password!='\0'?"\n":"");
}

static bool regular_identity(const char *path,artifact_identity *identity,char *error,size_t error_size) {
    struct stat status;if(stat(path,&status)!=0||!S_ISREG(status.st_mode)){
        (void)snprintf(error,error_size,"Required regular file is missing: %s",path);return false;}
    identity->size=status.st_size;identity->modified=status.st_mtime;
    return omt_sha256_file(path,identity->digest,error,error_size);
}

static bool same_identity(const char *path,const artifact_identity *identity) {
    struct stat status;char digest[65],error[64];
    return stat(path,&status)==0&&status.st_size==identity->size&&status.st_mtime==identity->modified&&
           omt_sha256_file(path,digest,error,sizeof(error))&&strcmp(digest,identity->digest)==0;
}

static bool digest_of(const char *output,char digest[65]) {
    if(output==NULL||strlen(output)<64U)return false;
    for(size_t i=0U;i<64U;++i){if(!isxdigit((unsigned char)output[i]))return false;digest[i]=(char)tolower((unsigned char)output[i]);}
    digest[64]='\0';return output[64]=='\0'||isspace((unsigned char)output[64])!=0;
}

static bool build_image(omt_deployment_service *service,const omt_deploy_options *o,char *error,size_t error_size) {
    char token[17],*stage,*output_arg,*build_arg;const char *args[17];artifact_identity identity;
    if(!o->build_image)return true;
    if(!omt_install_prerequisites(service,o->project_root,error,error_size))return false;
    EMIT_LITERAL(service,"Building the ARM64 appliance image...\n");if(!omt_random_token(8U,token,sizeof(token)))return false;
    stage=format("%s/.%s.%s.tmp",o->project_root,o->tarball_name,token);
    output_arg=format("type=docker,dest=%s",stage);build_arg=format("RPI_OMT_CLIENT_VERSION=%s",service->version);
    args[0]="docker";args[1]="buildx";args[2]="build";args[3]="--platform";args[4]="linux/arm64";
    args[5]="--build-arg";args[6]=build_arg;args[7]="--output";args[8]=output_arg;args[9]="--file";
    args[10]="deploy/Dockerfile";args[11]="-t";args[12]=o->image_name;args[13]=".";args[14]=NULL;
    bool ok=process_success(service,args,o->project_root,"ARM64 image build",error,error_size)&&
            regular_identity(stage,&identity,error,error_size)&&identity.size>=512;
    if(ok){char *final=join(o->project_root,o->tarball_name);
#ifdef _WIN32
        wchar_t *unused=NULL;(void)unused;ok=MoveFileExA(stage,final,MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH)!=0;
#else
        ok=rename(stage,final)==0;
#endif
        if(!ok)(void)snprintf(error,error_size,"Unable to publish the ARM64 image archive atomically.");
        free(final);}
    if(!ok)(void)remove(stage);
    free(stage);free(output_arg);free(build_arg);return ok;
}

bool omt_deploy(omt_deployment_service *service,const omt_connection *c,const omt_deploy_options *o,
                char *error,size_t error_size) {
    omt_string_list manifest={0};artifact_identity *identities=NULL;omt_ssh_client *ssh=NULL;
    char *manifest_path=NULL,*sudo_command=NULL,*sudo_data=NULL,*remote_q=NULL,*command=NULL;
    char token[25];bool ok=false;
    if(!checkpoint(service,error,error_size)||!omt_connection_validate(c,error,error_size)||
       !omt_options_validate(o,true,error,error_size))return false;
    service_secrets(service,c,NULL);
    manifest_path=format("%s/deploy/manifest-v3.txt",o->project_root);
    if(!omt_load_manifest(manifest_path,&manifest,error,error_size))goto done;
    identities=calloc(manifest.count,sizeof(*identities));if(identities==NULL)goto done;
    for(size_t i=0U;i<manifest.count;++i){char *local=join(o->project_root,manifest.items[i]);
        if(!(o->build_image&&strcmp(manifest.items[i],o->tarball_name)==0)&&!regular_identity(local,&identities[i],error,error_size)){free(local);goto done;}free(local);}
    if(!build_image(service,o,error,error_size))goto done;
    for(size_t i=0U;i<manifest.count;++i){char *local=join(o->project_root,manifest.items[i]);
        if(!regular_identity(local,&identities[i],error,error_size)){free(local);goto done;}free(local);}
    EMIT_LITERAL(service,"Connecting and checking the Raspberry Pi...\n");ssh=omt_ssh_connect(c,error,error_size);
    if(ssh==NULL||!platform(service,ssh,error,error_size))goto done;
    sudo_command=sudo_prefix(c);sudo_data=sudo_input(c);remote_q=omt_shell_quote(o->remote_directory);
    command=format("%s install -d -m 755 -o \"$(id -u)\" -g \"$(id -g)\" %s",sudo_command,remote_q);
    if(!remote_success(service,ssh,command,sudo_data,"Remote directory preparation",NULL,error,error_size))goto done;
    free(command);command=NULL;if(!omt_random_token(12U,token,sizeof(token)))goto done;
    {char *current=format("%s/deploy/transaction.sh",o->remote_directory);
     char *legacy=format("%s/deploy-transaction.sh",o->remote_directory);char *old_manifest=format("%s/deploy-artifacts.txt",o->remote_directory);
     char *cq=omt_shell_quote(current),*lq=omt_shell_quote(legacy),*mq=omt_shell_quote(old_manifest);
     command=format("if [ -x %s ] && [ -f %s ]; then %s recover %s %s; fi; if [ -x %s ]; then %s recover %s; fi",
                    lq,mq,lq,remote_q,mq,cq,cq,remote_q);
     free(current);free(legacy);free(old_manifest);free(cq);free(lq);free(mq);}
    if(!remote_success(service,ssh,command,"","Interrupted deployment recovery",NULL,error,error_size))goto done;
    free(command);command=NULL;
    {char *staging=format("%s/.deploy-staging",o->remote_directory),*stage=format("%s/%s",staging,token);
     char *sq=omt_shell_quote(staging),*stageq=omt_shell_quote(stage);
     command=format("if [ -L %s ] || { [ -e %s ] && [ ! -d %s ]; }; then exit 14; fi; install -d -m 700 -- %s; mkdir -- %s",sq,sq,sq,sq,stageq);
     if(!remote_success(service,ssh,command,"","Remote staging root validation",NULL,error,error_size)){free(staging);free(stage);free(sq);free(stageq);goto done;}
     free(command);command=NULL;
     for(size_t i=0U;i<manifest.count;++i){char *local=join(o->project_root,manifest.items[i]);char *remote=format("%s/%s",stage,manifest.items[i]);
        char *slash=strrchr(remote,'/');char digest[65];omt_remote_result checksum;
        if(slash!=NULL){*slash='\0';char *dq=omt_shell_quote(remote);command=format("mkdir -p -- %s",dq);*slash='/';free(dq);
            if(!remote_success(service,ssh,command,"","Remote staging preparation",NULL,error,error_size)){free(local);free(remote);goto stage_fail;}free(command);command=NULL;}
        {char *notice=format("Uploading %s...\n",manifest.items[i]);emit(service,notice,strlen(notice));free(notice);}
        if(!omt_ssh_upload(ssh,local,remote,NULL,service_stop,service,error,error_size)||!same_identity(local,&identities[i])){if(error[0]=='\0')(void)snprintf(error,error_size,"Local artifact changed while uploading: %s",manifest.items[i]);free(local);free(remote);goto stage_fail;}
        {char *rq=omt_shell_quote(remote);command=format("sha256sum -- %s",rq);free(rq);}
        if(!remote_success(service,ssh,command,"","Remote checksum",&checksum,error,error_size)){omt_remote_result_free(&checksum);free(local);free(remote);goto stage_fail;}
        if(!digest_of(checksum.output,digest)||strcmp(digest,identities[i].digest)!=0){(void)snprintf(error,error_size,"SHA-256 mismatch after uploading %s",manifest.items[i]);omt_remote_result_free(&checksum);free(local);free(remote);goto stage_fail;}
        omt_remote_result_free(&checksum);free(command);command=NULL;free(local);free(remote);
     }
     {char *helper=format("%s/deploy/transaction.sh",stage),*remote_manifest=format("%s/deploy/manifest-v3.txt",stage);
      char *hq=omt_shell_quote(helper),*mq=omt_shell_quote(remote_manifest),*tq=omt_shell_quote(token);
      command=format("bash %s promote %s %s %s",hq,remote_q,tq,mq);free(helper);free(remote_manifest);free(hq);free(mq);free(tq);}
     if(!remote_success(service,ssh,command,"","Deployment promotion",NULL,error,error_size))goto stage_fail;
     free(command);command=NULL;
     {const char *paths[]={"deploy/host/install.sh","deploy/host/uninstall.sh","deploy/host/host-diagnostics.sh",
       "deploy/host/host-event-watcher.sh","deploy/host/host-reboot.sh","deploy/transaction.sh"};
      char *chmod=strdup("chmod +x"),*install_path=NULL;
      for(size_t i=0U;i<6U;++i){char *path=join(o->remote_directory,paths[i]),*pq=omt_shell_quote(path),*grown=format("%s %s",chmod,pq);free(chmod);free(pq);chmod=grown;if(i==0U)install_path=path;else free(path);}
      char *iq=omt_shell_quote(install_path),*script=format("printf 'n\\n' | %s",iq),*scriptq=omt_shell_quote(script);
      command=format("%s && %s sh -c %s",chmod,sudo_command,scriptq);free(chmod);free(install_path);free(iq);free(script);free(scriptq);}
     if(!remote_success(service,ssh,command,sudo_data,"Remote installer",NULL,error,error_size))goto stage_fail;
     EMIT_LITERAL(service,"Deployment completed successfully.\n");ok=true;goto stage_done;
stage_fail:
     free(command);command=NULL;{char *stageq2=omt_shell_quote(stage);command=format("if [ -d %s ] && [ ! -L %s ]; then find -P %s -xdev -depth -delete; fi",stageq2,stageq2,stageq2);free(stageq2);}
     {omt_remote_result ignored;(void)omt_ssh_run(ssh,command,"",NULL,NULL,NULL,&ignored,NULL,0U);omt_remote_result_free(&ignored);}
stage_done:free(command);command=NULL;free(staging);free(stage);free(sq);free(stageq);}
done:
    free(command);free(manifest_path);free(identities);free(sudo_command);if(sudo_data!=NULL){omt_secure_clear(sudo_data,strlen(sudo_data));free(sudo_data);}free(remote_q);
    if(ssh!=NULL)omt_ssh_close(ssh);
    omt_string_list_free(&manifest);return ok;
}

bool omt_manage(omt_deployment_service *service,const omt_connection *c,const char *directory,
                const char *action,char **output,char *error,size_t error_size) {
    omt_ssh_client *ssh;omt_remote_result result;char *dq,*command;bool ok;
    if(output==NULL||!checkpoint(service,error,error_size)||!omt_connection_validate(c,error,error_size)||
       !omt_valid_remote_directory(directory))return false;
    service_secrets(service,c,NULL);*output=NULL;
    ssh=omt_ssh_connect(c,error,error_size);if(ssh==NULL)return false;dq=omt_shell_quote(directory);
    command=format("cd %s && %s",dq,action);ok=remote_success(service,ssh,command,"","Remote management action",&result,error,error_size);
    if(ok){*output=result.output;result.output=NULL;}omt_remote_result_free(&result);free(dq);free(command);omt_ssh_close(ssh);return ok;
}

bool omt_apply_wifi(omt_deployment_service *service,const omt_connection *c,const omt_wifi_settings *w,
                    char *error,size_t error_size) {
    static const char *script="marker=$4; while IFS= read -r line; do [ \"$line\" = \"$marker\" ] && break; done; IFS= read -r pass; ssid=$1; ssid_hex=$2; activate=$3; raw_psk=$5; command -v wpa_cli >/dev/null; wpa_cli -i wlan0 ping | grep -Fxq PONG; if [ \"$raw_psk\" = yes ]; then psk=$(printf '%s' \"$pass\" | tr 'A-F' 'a-f'); else command -v wpa_passphrase >/dev/null; psk=$(printf '%s\\n' \"$pass\" | wpa_passphrase \"$ssid\" | sed -n 's/^[[:space:]]*psk=//p' | tail -n1); fi; unset pass; [ ${#psk} -eq 64 ]; id=; for candidate in $(wpa_cli -i wlan0 list_networks | awk 'NR>2 {print $1}'); do [ \"$(wpa_cli -i wlan0 get_network \"$candidate\" ssid 2>/dev/null)\" = \"$ssid_hex\" ] && id=$candidate && break; done; [ -n \"$id\" ] || id=$(wpa_cli -i wlan0 add_network); case $id in ''|*[!0-9]*) exit 13;; esac; wpa_cli -i wlan0 set_network \"$id\" ssid \"$ssid_hex\" | grep -Fxq OK; wpa_cli -i wlan0 set_network \"$id\" key_mgmt WPA-PSK | grep -Fxq OK; wpa_cli -i wlan0 set_network \"$id\" psk \"$psk\" | grep -Fxq OK; unset psk; wpa_cli -i wlan0 enable_network \"$id\" | grep -Fxq OK; wpa_cli -i wlan0 save_config | grep -Fxq OK; [ \"$activate\" = no ] || { wpa_cli -i wlan0 select_network \"$id\" >/dev/null; wpa_cli -i wlan0 reassociate >/dev/null; }";
    char token[25],marker[48],ssid_hex[65],*sudo_command,*sudo_data,*input,*sq,*ssidq,*markerq,*command;bool raw=true,ok;
    omt_ssh_client *ssh=NULL;
    if(!checkpoint(service,error,error_size)||!omt_connection_validate(c,error,error_size)||!omt_wifi_validate(w,error,error_size))return false;
    service_secrets(service,c,w->password);if(!omt_random_token(12U,token,sizeof(token)))return false;
    (void)snprintf(marker,sizeof(marker),"__OMT_WIFI_PASSWORD_%s__",token);
    for(size_t i=0U;i<strlen(w->ssid);++i)(void)snprintf(ssid_hex+i*2U,3U,"%02x",(unsigned char)w->ssid[i]);
    for(size_t i=0U;i<strlen(w->password);++i)if(!isxdigit((unsigned char)w->password[i]))raw=false;
    raw=raw&&strlen(w->password)==64U;sudo_command=sudo_prefix(c);sudo_data=sudo_input(c);
    input=NULL;sq=NULL;ssidq=NULL;markerq=NULL;command=NULL;ok=false;
    if(sudo_command==NULL||sudo_data==NULL){
        if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Unable to prepare Wi-Fi authentication input.");
        goto wifi_done;
    }
    input=format("%s%s\n%s\n",sudo_data,marker,w->password);sq=omt_shell_quote(script);ssidq=omt_shell_quote(w->ssid);markerq=omt_shell_quote(marker);
    if(input==NULL||sq==NULL||ssidq==NULL||markerq==NULL){
        if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Unable to allocate Wi-Fi update command buffers.");
        goto wifi_done;
    }
    command=format("%s -v && sudo -n sh -eu -c %s sh %s '%s' %s %s %s",sudo_command,sq,ssidq,ssid_hex,
                   w->connect?"yes":"no",markerq,raw?"yes":"no");
    if(command==NULL){
        if(error!=NULL&&error_size>0U)(void)snprintf(error,error_size,"Unable to allocate Wi-Fi update command buffers.");
        goto wifi_done;
    }
    ssh=omt_ssh_connect(c,error,error_size);ok=ssh!=NULL&&remote_success(service,ssh,command,input,"Wi-Fi update",NULL,error,error_size);
    if(ssh!=NULL)omt_ssh_close(ssh);
wifi_done:
    if(input!=NULL){omt_secure_clear(input,strlen(input));free(input);}
    if(sudo_data!=NULL){omt_secure_clear(sudo_data,strlen(sudo_data));free(sudo_data);}
    free(sudo_command);free(sq);free(ssidq);free(markerq);free(command);return ok;
}
