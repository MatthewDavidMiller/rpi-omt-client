#define _POSIX_C_SOURCE 200809L
#include "deployer.h"

#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>

#ifdef _WIN32
#include <direct.h>
#define make_directory(path) _mkdir(path)
#define remove_directory(path) _rmdir(path)
#define PATH_SEPARATOR '\\'
#else
#include <unistd.h>
#define make_directory(path) mkdir((path),0700)
#define remove_directory(path) rmdir(path)
#define PATH_SEPARATOR '/'
#endif

static void fill(char *output,size_t count,char value) {
    memset(output,value,count);output[count]='\0';
}

static bool manifest_accepted(const char *path,const char *body) {
    FILE *file=fopen(path,"wb");omt_string_list list={0};char error[256];bool accepted;
    assert(file!=NULL);assert(fwrite(body,1U,strlen(body),file)==strlen(body));assert(fclose(file)==0);
    accepted=omt_load_manifest(path,&list,error,sizeof(error));omt_string_list_free(&list);return accepted;
}

// Manifest v3 names every path the deployer uploads and transaction.sh then
// promotes into the install directory as root. A name that escaped this
// validation would be written outside the staging tree on the Pi, so each
// rejection below is a security boundary rather than a formatting preference.
static void manifest_contract(const char *path,const char *directory) {
    const char *required="version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n";
    const char *unsafe[]={"../etc/passwd","deploy/../../etc/passwd","/etc/passwd","deploy//transaction.sh",
        "deploy/","/",".","..","deploy/./transaction.sh","deploy\\transaction.sh","deploy/trans action.sh",
        "deploy/transaction.sh\r","deploy/caf\xc3\xa9.sh"};
    assert(manifest_accepted(path,required));
    assert(manifest_accepted(path,"version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\ndeploy/host/install.sh\n"));
    // The version line is the schema gate; a v2 capsule must not be read as v3.
    assert(!manifest_accepted(path,"version=2\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_accepted(path,"version=3 \ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_accepted(path,"deploy/transaction.sh\ndeploy/manifest-v3.txt\n"));
    assert(!manifest_accepted(path,""));
    // Both members the transaction itself needs must be present.
    assert(!manifest_accepted(path,"version=3\n"));
    assert(!manifest_accepted(path,"version=3\ndeploy/transaction.sh\n"));
    assert(!manifest_accepted(path,"version=3\ndeploy/manifest-v3.txt\n"));
    for(size_t i=0U;i<sizeof(unsafe)/sizeof(unsafe[0]);++i){
        char body[1024];(void)snprintf(body,sizeof(body),"%s%s\n",required,unsafe[i]);assert(!manifest_accepted(path,body));}
    // A duplicate would be uploaded and hashed twice with no way to tell which
    // copy the promotion used, and no single name may be unbounded.
    assert(!manifest_accepted(path,"version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\ndeploy/transaction.sh\n"));
    {char body[1024];size_t used=(size_t)snprintf(body,sizeof(body),"%s",required);
     memset(body+used,'x',241U);body[used+241U]='\n';body[used+242U]='\0';assert(!manifest_accepted(path,body));}
    // The capsule is bounded, so a manifest cannot ask for an unbounded upload.
    {char body[32768];size_t used=(size_t)snprintf(body,sizeof(body),"%s",required);
     for(unsigned i=0U;i<200U;++i)used+=(size_t)snprintf(body+used,sizeof(body)-used,"deploy/file%u.txt\n",i);
     assert(!manifest_accepted(path,body));}
    // A manifest that is missing or is not a plain regular file is unusable.
    {omt_string_list list={0};char error[256];assert(!omt_load_manifest(directory,&list,error,sizeof(error)));}
    assert(remove(path)==0);{omt_string_list list={0};char error[256];assert(!omt_load_manifest(path,&list,error,sizeof(error)));}
}

static bool immediate_stop(void *context){(void)context;return true;}

static bool contains_bytes(const char *text,size_t text_size,const char *needle) {
    const size_t needle_size=strlen(needle);
    if(needle_size>text_size)return false;
    for(size_t i=0U;i<=text_size-needle_size;++i)
        if(memcmp(text+i,needle,needle_size)==0)return true;
    return false;
}

static void path_append(char *output,size_t capacity,const char *base,const char *suffix) {
    assert(strlen(base)+strlen(suffix)+1U<=capacity);
    strcpy(output,base);
    strcat(output,suffix);
}

int main(void) {
    char token[33],root[4096],deploy[4096],manifest[4096],sample[4096],digest[65],error[256];
    char oversized[4098];
    assert(omt_valid_host("pi.local"));assert(omt_valid_host("192.168.1.20"));
    assert(!omt_valid_host("-pi.local"));assert(!omt_valid_host("pi..local"));assert(!omt_valid_host(""));
    assert(!omt_valid_host("pi.local."));assert(!omt_valid_host("pi local"));
    fill(oversized,254U,'a');assert(!omt_valid_host(oversized));
    assert(omt_valid_username("pi_admin-1"));assert(!omt_valid_username("pi admin"));
    assert(!omt_valid_username(""));assert(!omt_valid_username("pi/admin"));
    fill(oversized,65U,'a');assert(!omt_valid_username(oversized));
    assert(omt_valid_remote_directory("/opt/omt-client"));assert(!omt_valid_remote_directory("/opt/../root"));
    assert(!omt_valid_remote_directory("/"));assert(!omt_valid_remote_directory("opt/omt-client"));
    assert(!omt_valid_remote_directory("/opt/omt-client/"));assert(!omt_valid_remote_directory("/opt//omt-client"));
    assert(!omt_valid_remote_directory("/opt/omt client"));
    // Every field the deployer forwards to libssh2 or interpolates into a
    // remote command is bounded and control-character free before it is used.
    {omt_connection c={"pi.local","admin",22U,OMT_AUTH_PASSWORD,"password",NULL,NULL,NULL};
     assert(omt_connection_validate(&c,error,sizeof(error)));
     c.port=0U;assert(!omt_connection_validate(&c,error,sizeof(error)));c.port=22U;
     fill(oversized,4097U,'x');c.password=oversized;assert(!omt_connection_validate(&c,error,sizeof(error)));
     c.password="password";c.sudo_password="with\nnewline";assert(!omt_connection_validate(&c,error,sizeof(error)));
     c.sudo_password=NULL;c.host="pi local";assert(!omt_connection_validate(&c,error,sizeof(error)));}
    // Single quoting is the only escape a POSIX shell honours inside '...', so
    // every argument the deployer interpolates into a remote command relies on
    // this exact form.
    {const char *cases[][2]={{"a'b","'a'\\''b'"},{"","''"},{"plain","'plain'"},
        {"$(id)","'$(id)'"},{"a b;rm -rf /","'a b;rm -rf /'"}};
     for(size_t i=0U;i<sizeof(cases)/sizeof(cases[0]);++i){char *quoted=omt_shell_quote(cases[i][0]);
        assert(quoted!=NULL&&strcmp(quoted,cases[i][1])==0);free(quoted);}}
    assert(omt_random_token(16U,token,sizeof(token)));assert(strlen(token)==32U);
    assert(strspn(token,"0123456789abcdef")==32U);
    {char second[33];assert(omt_random_token(16U,second,sizeof(second)));assert(strcmp(second,token)!=0);}
#ifdef _WIN32
    {const char *temporary=getenv("TEMP");assert(temporary!=NULL);
     path_append(root,sizeof(root),temporary,"\\omt-deployer-core-test-");assert(strlen(root)+strlen(token)+1U<=sizeof(root));strcat(root,token);
     path_append(deploy,sizeof(deploy),root,"\\deploy");path_append(manifest,sizeof(manifest),deploy,"\\manifest-v3.txt");
     path_append(sample,sizeof(sample),root,"\\sample.txt");}
#else
    (void)snprintf(root,sizeof(root),"/tmp/omt-deployer-core-test-%s",token);
    path_append(deploy,sizeof(deploy),root,"/deploy");path_append(manifest,sizeof(manifest),deploy,"/manifest-v3.txt");
    path_append(sample,sizeof(sample),root,"/sample.txt");
#endif
    assert(make_directory(root)==0);assert(make_directory(deploy)==0);manifest_contract(manifest,deploy);
    {FILE *file=fopen(manifest,"wb");assert(file!=NULL);assert(fputs("version=3\ndeploy/transaction.sh\ndeploy/manifest-v3.txt\n",file)>=0);fclose(file);}
    {omt_string_list list={0};assert(omt_load_manifest(manifest,&list,error,sizeof(error)));assert(list.count==2U);omt_string_list_free(&list);}
    {FILE *file=fopen(sample,"wb");assert(file!=NULL);assert(fwrite("abc",1U,3U,file)==3U);fclose(file);}
    assert(omt_sha256_file(sample,digest,error,sizeof(error)));
    assert(strcmp(digest,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")==0);
    // The deployer is started from an installed package or a desktop shortcut
    // as often as from a checkout, so it locates the tree it uploads itself.
    {char *found=omt_discover_project_root(root,root);assert(found!=NULL&&strcmp(found,root)==0);free(found);}
    {char *found=omt_discover_project_root(root,NULL);assert(found!=NULL&&strcmp(found,root)==0);free(found);}
    {char *found=omt_discover_project_root(NULL,root);assert(found!=NULL&&strcmp(found,root)==0);free(found);}
    {char *found=omt_discover_project_root("","");assert(found!=NULL&&*found=='\0');free(found);}
    {char nested_parent[4096],nested[4096],trailing[4096],*found;
#ifdef _WIN32
     path_append(nested_parent,sizeof(nested_parent),root,"\\nested");
     path_append(nested,sizeof(nested),nested_parent,"\\bin");
     path_append(trailing,sizeof(trailing),nested,"\\");
#else
     path_append(nested_parent,sizeof(nested_parent),root,"/nested");
     path_append(nested,sizeof(nested),nested_parent,"/bin");
     path_append(trailing,sizeof(trailing),nested,"/");
#endif
     assert(make_directory(nested_parent)==0);assert(make_directory(nested)==0);
     // SDL reports the executable's directory with a trailing separator, which
     // must not stall the ascent on its first step.
     found=omt_discover_project_root(trailing,NULL);
     assert(found!=NULL&&strcmp(found,root)==0);free(found);
     found=omt_discover_project_root(nested,NULL);
     assert(found!=NULL&&strcmp(found,root)==0);free(found);
     assert(remove_directory(nested)==0);assert(remove_directory(nested_parent)==0);}
    // The ascent is bounded, so a marker far above an unrelated directory is
    // not adopted as the project root.
    {char deep[4096];size_t used=(size_t)snprintf(deep,sizeof(deep),"%s",root);char *found;
     for(unsigned level=0U;level<10U;++level){
        used+=(size_t)snprintf(deep+used,sizeof(deep)-used,"%clevel",PATH_SEPARATOR);
        assert(make_directory(deep)==0);}
     found=omt_discover_project_root(NULL,deep);assert(found!=NULL&&strcmp(found,deep)==0);free(found);
     for(unsigned level=0U;level<10U;++level){assert(remove_directory(deep)==0);*strrchr(deep,PATH_SEPARATOR)='\0';}}
    // The legal texts are compiled in, so About cannot be left with nothing to
    // show by a package that travelled without its files.
    {const omt_legal_document *documents=NULL;const size_t count=omt_legal_documents(&documents);
     assert(count==2U);assert(strcmp(documents[0].name,"LICENSE")==0);
     assert(contains_bytes(documents[0].text,documents[0].text_size,"MIT License"));
     assert(contains_bytes(documents[0].text,documents[0].text_size,"Matthew David Miller"));
     assert(strcmp(documents[1].name,"THIRD_PARTY_NOTICES.txt")==0);
     assert(contains_bytes(documents[1].text,documents[1].text_size,"Nuklear 4.13.3"));
     // Text, not a truncated or NUL-padded blob: About renders it verbatim.
     for(size_t i=0U;i<count;++i){assert(documents[i].text_size>512U);
        assert(memchr(documents[i].text,'\0',documents[i].text_size)==NULL);
        assert(documents[i].text[documents[i].text_size-1U]=='\n');}}
    // WPA2 accepts either an 8-63 character passphrase or a 64-digit hex PSK;
    // anything else is rejected here rather than by wpa_supplicant on a Pi that
    // has just lost its network.
    {omt_wifi_settings wifi={"office","12345678",true};char secret[66];
     assert(omt_wifi_validate(&wifi,error,sizeof(error)));
     fill(secret,63U,'p');wifi.password=secret;assert(omt_wifi_validate(&wifi,error,sizeof(error)));
     fill(secret,64U,'a');assert(omt_wifi_validate(&wifi,error,sizeof(error)));
     fill(secret,64U,'z');assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     fill(secret,65U,'a');assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     wifi.password="short";assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     wifi.password="with\nnewline";assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     wifi.password="12345678";wifi.ssid="";assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     fill(secret,33U,'s');wifi.ssid=secret;assert(!omt_wifi_validate(&wifi,error,sizeof(error)));
     wifi.ssid="bad\xC0\xAF";assert(!omt_wifi_validate(&wifi,error,sizeof(error)));}
#ifdef _WIN32
    {const char *success[]={"cmd.exe","/d","/s","/c","echo ok",NULL};
     const char *cancel[]={"cmd.exe","/d","/s","/c","ping -n 30 127.0.0.1 >nul",NULL};
#else
    {const char *success[]={"/bin/sh","-c","printf ok",NULL};
     const char *cancel[]={"/bin/sh","-c","while :; do sleep 1; done",NULL};
#endif
     omt_process_result result;assert(omt_run_process(success,root,NULL,NULL,NULL,&result,error,sizeof(error)));
     assert(result.exit_code==0&&strstr(result.output,"ok")!=NULL);omt_process_result_free(&result);
     assert(!omt_run_process(cancel,root,NULL,immediate_stop,NULL,&result,error,sizeof(error)));omt_process_result_free(&result);}
    assert(remove(sample)==0);assert(remove(manifest)==0);assert(remove_directory(deploy)==0);assert(remove_directory(root)==0);
    puts("native deployer core contracts passed");return 0;
}
