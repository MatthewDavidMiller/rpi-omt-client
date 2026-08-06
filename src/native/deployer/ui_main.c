#ifndef _WIN32
#define _POSIX_C_SOURCE 200809L
#endif
#include "deployer.h"

#include <SDL3/SDL.h>
#include <SDL3/SDL_main.h>

#define NK_INCLUDE_STANDARD_VARARGS
#define NK_INCLUDE_STANDARD_IO
#define NK_INCLUDE_FONT_BAKING
#define NK_INCLUDE_DEFAULT_FONT
#define NK_INCLUDE_COMMAND_USERDATA
#define NK_INCLUDE_VERTEX_BUFFER_OUTPUT
#define NK_INCLUDE_DEFAULT_ALLOCATOR
#include <nuklear.h>
#include <nuklear_sdl3_renderer.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <direct.h>
#define working_directory(buffer, size) _getcwd((buffer), (int)(size))
#else
#include <unistd.h>
#define working_directory(buffer, size) getcwd((buffer), (size))
#endif

enum operation {
    OP_NONE, OP_TEST, OP_PREREQUISITES, OP_DEPLOY, OP_STATUS, OP_LOGS, OP_RESTART, OP_WIFI
};

typedef struct {
    char host[256],username[65],port[6],password[512],key_path[1024],key_passphrase[512];
    char sudo_password[512],project_root[2048],remote_directory[256],wifi_ssid[64],wifi_password[128];
    bool use_key,build_image,wifi_connect;
    SDL_Thread *thread;
    SDL_Mutex *mutex;
    SDL_AtomicInt cancel;
    SDL_AtomicInt working;
    enum operation operation;
    char *log;
    size_t log_size;
    int tab;
} application;

static void secure_buffer(char *buffer,size_t size) {
    omt_secure_clear(buffer,size);
}

static void append_log(application *app,const char *text,size_t size) {
    SDL_LockMutex(app->mutex);
    if(size>OMT_DEPLOYER_OUTPUT_LIMIT){text+=size-OMT_DEPLOYER_OUTPUT_LIMIT;size=OMT_DEPLOYER_OUTPUT_LIMIT;}
    if(app->log_size+size>OMT_DEPLOYER_OUTPUT_LIMIT){
        const size_t remove=app->log_size+size-OMT_DEPLOYER_OUTPUT_LIMIT;
        memmove(app->log,app->log+remove,app->log_size-remove);app->log_size-=remove;
    }
    char *grown=realloc(app->log,app->log_size+size+1U);
    if(grown!=NULL){app->log=grown;memcpy(app->log+app->log_size,text,size);app->log_size+=size;app->log[app->log_size]='\0';}
    SDL_UnlockMutex(app->mutex);
}
#define APPEND_LITERAL(app, text) append_log((app),(text),sizeof(text)-1U)

static void event_log(const char *text,size_t size,void *context){append_log(context,text,size);}
static bool cancelled(void *context){return SDL_GetAtomicInt(&((application *)context)->cancel)!=0;}

static omt_connection connection_of(application *app) {
    omt_connection c={app->host,app->username,0U,app->use_key?OMT_AUTH_KEY:OMT_AUTH_PASSWORD,
        app->password,app->key_path,app->key_passphrase,app->sudo_password};
    char *end=NULL;const unsigned long port=strtoul(app->port,&end,10);
    c.port=end!=app->port&&*end=='\0'&&port<=65535UL?(uint16_t)port:0U;return c;
}

static omt_deploy_options options_of(application *app) {
    omt_deploy_options o={app->project_root,app->remote_directory,"omt-client","omt-client-arm64.tar.gz",app->build_image};
    return o;
}

static int worker(void *context) {
    application *app=context;char error[OMT_DEPLOYER_ERROR_SIZE]={0};char *output=NULL;bool ok=false;
    omt_connection connection=connection_of(app);omt_deploy_options options=options_of(app);
    omt_wifi_settings wifi={app->wifi_ssid,app->wifi_password,app->wifi_connect};
    omt_deployment_service *service=omt_deployment_service_create(OMT_CLIENT_VERSION,event_log,cancelled,app);
    if(service==NULL)(void)snprintf(error,sizeof(error),"Unable to allocate deployment service.");
    else switch(app->operation){
        case OP_TEST:ok=omt_test_connection(service,&connection,error,sizeof(error));break;
        case OP_PREREQUISITES:ok=omt_install_prerequisites(service,app->project_root,error,sizeof(error));break;
        case OP_DEPLOY:ok=omt_deploy(service,&connection,&options,error,sizeof(error));break;
        case OP_STATUS:ok=omt_manage(service,&connection,app->remote_directory,"docker compose -f deploy/compose.yml ps",&output,error,sizeof(error));break;
        case OP_LOGS:ok=omt_manage(service,&connection,app->remote_directory,"docker compose -f deploy/compose.yml logs --tail=120",&output,error,sizeof(error));break;
        case OP_RESTART:ok=omt_manage(service,&connection,app->remote_directory,"docker compose -f deploy/compose.yml restart",&output,error,sizeof(error));break;
        case OP_WIFI:ok=omt_apply_wifi(service,&connection,&wifi,error,sizeof(error));break;
        case OP_NONE:break;
    }
    if(output!=NULL){append_log(app,output,strlen(output));if(*output!='\0'&&output[strlen(output)-1U]!='\n')append_log(app,"\n",1U);free(output);}
    if(ok)APPEND_LITERAL(app,"Operation completed.\n");else APPEND_LITERAL(app,"ERROR: ");
    if(!ok){append_log(app,error,strlen(error));append_log(app,"\n",1U);}
    omt_deployment_service_destroy(service);SDL_SetAtomicInt(&app->working,0);return ok?0:1;
}

static void start(application *app,enum operation operation) {
    if(SDL_GetAtomicInt(&app->working)!=0)return;
    if(app->thread!=NULL){SDL_WaitThread(app->thread,NULL);app->thread=NULL;}
    APPEND_LITERAL(app,"\n--- Starting operation ---\n");app->operation=operation;
    SDL_SetAtomicInt(&app->cancel,0);SDL_SetAtomicInt(&app->working,1);
    app->thread=SDL_CreateThread(worker,"omt-deployer-operation",app);
    if(app->thread==NULL){SDL_SetAtomicInt(&app->working,0);APPEND_LITERAL(app,"ERROR: Unable to start operation thread.\n");}
}

static void edit(struct nk_context *ctx,const char *label,char *value,int capacity,bool secret,bool enabled) {
    struct nk_style_edit saved;
    nk_layout_row_dynamic(ctx,25.0F,2);nk_label(ctx,label,NK_TEXT_LEFT);
    if(!enabled)nk_widget_disable_begin(ctx);
    if(secret){
        saved=ctx->style.edit;
        ctx->style.edit.text_normal.a=0U;ctx->style.edit.text_hover.a=0U;ctx->style.edit.text_active.a=0U;
        ctx->style.edit.selected_text_normal.a=0U;ctx->style.edit.selected_text_hover.a=0U;
        ctx->style.edit.cursor_text_normal.a=0U;ctx->style.edit.cursor_text_hover.a=0U;
    }
    (void)nk_edit_string_zero_terminated(ctx,NK_EDIT_FIELD,value,capacity,nk_filter_default);
    if(secret)ctx->style.edit=saved;
    if(!enabled)nk_widget_disable_end(ctx);
}

static void draw_connection(struct nk_context *ctx,application *app,bool idle) {
    edit(ctx,"Pi host",app->host,(int)sizeof(app->host),false,idle);
    edit(ctx,"SSH username",app->username,(int)sizeof(app->username),false,idle);
    edit(ctx,"SSH port",app->port,(int)sizeof(app->port),false,idle);
    nk_layout_row_dynamic(ctx,25.0F,1);if(idle)app->use_key=nk_check_label(ctx,"Use private key",app->use_key)!=0;
    if(app->use_key){edit(ctx,"Private key",app->key_path,(int)sizeof(app->key_path),false,idle);
        edit(ctx,"Key passphrase",app->key_passphrase,(int)sizeof(app->key_passphrase),true,idle);}
    else edit(ctx,"SSH password",app->password,(int)sizeof(app->password),true,idle);
    edit(ctx,"Sudo password (optional)",app->sudo_password,(int)sizeof(app->sudo_password),true,idle);
    nk_layout_row_dynamic(ctx,30.0F,1);if(idle&&nk_button_label(ctx,"Test connection"))start(app,OP_TEST);
    nk_layout_row_dynamic(ctx,35.0F,1);nk_label_wrap(ctx,"Host keys must already exist in ~/.ssh/known_hosts. Unknown or changed keys are refused.");
}

static void draw_deployment(struct nk_context *ctx,application *app,bool idle) {
    edit(ctx,"Project root",app->project_root,(int)sizeof(app->project_root),false,idle);
    edit(ctx,"Remote directory",app->remote_directory,(int)sizeof(app->remote_directory),false,idle);
    nk_layout_row_dynamic(ctx,25.0F,1);if(idle)app->build_image=nk_check_label(ctx,"Build ARM64 image",app->build_image)!=0;
    nk_layout_row_dynamic(ctx,30.0F,2);
    if(idle&&nk_button_label(ctx,"Install prerequisites"))start(app,OP_PREREQUISITES);
    if(idle&&nk_button_label(ctx,"Build and deploy"))start(app,OP_DEPLOY);
    nk_layout_row_dynamic(ctx,30.0F,3);
    if(idle&&nk_button_label(ctx,"Status"))start(app,OP_STATUS);
    if(idle&&nk_button_label(ctx,"Recent logs"))start(app,OP_LOGS);
    if(idle&&nk_button_label(ctx,"Restart service"))start(app,OP_RESTART);
}

static void draw_wifi(struct nk_context *ctx,application *app,bool idle) {
    edit(ctx,"SSID",app->wifi_ssid,(int)sizeof(app->wifi_ssid),false,idle);
    edit(ctx,"Wi-Fi password",app->wifi_password,(int)sizeof(app->wifi_password),true,idle);
    nk_layout_row_dynamic(ctx,25.0F,1);if(idle)app->wifi_connect=nk_check_label(ctx,"Connect immediately",app->wifi_connect)!=0;
    nk_layout_row_dynamic(ctx,30.0F,1);if(idle&&nk_button_label(ctx,"Apply Wi-Fi settings"))start(app,OP_WIFI);
    nk_layout_row_dynamic(ctx,30.0F,1);nk_label_wrap(ctx,"Connecting can interrupt SSH if the Pi changes networks.");
}

static void draw_about(struct nk_context *ctx) {
    const omt_legal_document *documents=NULL;const size_t count=omt_legal_documents(&documents);
    char heading[256];(void)snprintf(heading,sizeof(heading),"Raspberry Pi OMT Client %s",OMT_CLIENT_VERSION);
    nk_layout_row_dynamic(ctx,20.0F,1);nk_label(ctx,heading,NK_TEXT_LEFT);
    nk_label(ctx,"Copyright (c) 2026 Matthew David Miller",NK_TEXT_LEFT);
    nk_label_wrap(ctx,"Native C17 deployer using SDL3, Nuklear, and libssh2. Project code is MIT licensed.");
    for(size_t i=0U;i<count;++i){(void)snprintf(heading,sizeof(heading),"===== %s =====",documents[i].name);
        nk_layout_row_dynamic(ctx,20.0F,1);nk_label(ctx,heading,NK_TEXT_LEFT);
        const size_t shown=documents[i].text_size;
        char *text=malloc(shown+1U);if(text!=NULL){memcpy(text,documents[i].text,shown);text[shown]='\0';
            nk_layout_row_dynamic(ctx,(float)(shown/80U+1U)*16.0F,1);nk_label_wrap(ctx,text);free(text);}}
}

static void draw_ui(struct nk_context *ctx,application *app,int width,int height) {
    const bool idle=SDL_GetAtomicInt(&app->working)==0;
    if(nk_begin(ctx,"Raspberry Pi OMT Deployer",nk_rect(0.0F,0.0F,(float)width,(float)height),
                NK_WINDOW_BORDER|NK_WINDOW_NO_SCROLLBAR)){
        char title[128];(void)snprintf(title,sizeof(title),"Raspberry Pi OMT Deployer — native %s",OMT_CLIENT_VERSION);
        nk_layout_row_dynamic(ctx,25.0F,1);nk_label(ctx,title,NK_TEXT_LEFT);
        nk_layout_row_dynamic(ctx,28.0F,4);
        if(nk_option_label(ctx,"Connection",app->tab==0))app->tab=0;
        if(nk_option_label(ctx,"Deployment",app->tab==1))app->tab=1;
        if(nk_option_label(ctx,"Wi-Fi",app->tab==2))app->tab=2;
        if(nk_option_label(ctx,"About",app->tab==3))app->tab=3;
        nk_layout_row_dynamic(ctx,(float)height*0.48F,1);
        if(nk_group_begin(ctx,"workspace",NK_WINDOW_BORDER)){
            if(app->tab==0)draw_connection(ctx,app,idle);else if(app->tab==1)draw_deployment(ctx,app,idle);
            else if(app->tab==2)draw_wifi(ctx,app,idle);else draw_about(ctx);
            nk_group_end(ctx);}
        nk_layout_row_dynamic(ctx,30.0F,2);
        if(!idle&&nk_button_label(ctx,"Cancel operation")){SDL_SetAtomicInt(&app->cancel,1);APPEND_LITERAL(app,"Cancellation requested; waiting for a safe boundary.\n");}
        if(nk_button_label(ctx,"Copy activity log")){SDL_LockMutex(app->mutex);(void)SDL_SetClipboardText(app->log==NULL?"":app->log);SDL_UnlockMutex(app->mutex);}
        nk_layout_row_dynamic(ctx,(float)height*0.30F,1);
        if(nk_group_begin(ctx,"Activity",NK_WINDOW_BORDER)){SDL_LockMutex(app->mutex);
            nk_label_wrap(ctx,app->log==NULL?"Ready.\n":app->log);SDL_UnlockMutex(app->mutex);nk_group_end(ctx);}
    }nk_end(ctx);
}

int main(int argc,char **argv) {
    SDL_Window *window=NULL;SDL_Renderer *renderer=NULL;struct nk_context *ctx;application app;
    bool running=true;int width=1080,height=760;(void)argc;(void)argv;memset(&app,0,sizeof(app));
    (void)snprintf(app.port,sizeof(app.port),"22");(void)snprintf(app.username,sizeof(app.username),"admin");
    (void)snprintf(app.remote_directory,sizeof(app.remote_directory),"/opt/omt-client");app.build_image=true;app.wifi_connect=true;
    app.mutex=SDL_CreateMutex();app.log=strdup("Ready.\n");app.log_size=app.log==NULL?0U:strlen(app.log);
    if(app.mutex==NULL||!SDL_Init(SDL_INIT_VIDEO)||!SDL_CreateWindowAndRenderer("Raspberry Pi OMT Deployer",width,height,
       SDL_WINDOW_RESIZABLE|SDL_WINDOW_HIGH_PIXEL_DENSITY,&window,&renderer)){
        SDL_ShowSimpleMessageBox(SDL_MESSAGEBOX_ERROR,"Raspberry Pi OMT Deployer",SDL_GetError(),NULL);return 1;}
    /* SDL owns the base path it returns; the working directory is searched first
       so a packaged deployer started inside a checkout still finds that tree. */
    {const char *base=SDL_GetBasePath();char working[2048];char *root;
     if(working_directory(working,sizeof(working))==NULL)working[0]='\0';
     root=omt_discover_project_root(base,working);
     if(root!=NULL){(void)snprintf(app.project_root,sizeof(app.project_root),"%s",root);free(root);}}
    (void)SDL_SetWindowMinimumSize(window,720,520);(void)SDL_SetRenderVSync(renderer,1);
    ctx=nk_sdl_init(window,renderer,nk_sdl_allocator());nk_sdl_style_set_debug_font(ctx);nk_input_begin(ctx);
    while(running){SDL_Event event;while(SDL_PollEvent(&event)){if(event.type==SDL_EVENT_QUIT||event.type==SDL_EVENT_WINDOW_CLOSE_REQUESTED)running=false;
            (void)SDL_ConvertEventToRenderCoordinates(renderer,&event);(void)nk_sdl_handle_event(ctx,&event);}
        nk_input_end(ctx);(void)SDL_GetWindowSize(window,&width,&height);draw_ui(ctx,&app,width,height);
        (void)SDL_SetRenderDrawColor(renderer,15U,23U,42U,255U);(void)SDL_RenderClear(renderer);
        nk_sdl_render(ctx,NK_ANTI_ALIASING_ON);nk_sdl_update_TextInput(ctx);SDL_RenderPresent(renderer);nk_input_begin(ctx);}
    SDL_SetAtomicInt(&app.cancel,1);if(app.thread!=NULL)SDL_WaitThread(app.thread,NULL);
    secure_buffer(app.password,sizeof(app.password));secure_buffer(app.key_passphrase,sizeof(app.key_passphrase));
    secure_buffer(app.sudo_password,sizeof(app.sudo_password));secure_buffer(app.wifi_password,sizeof(app.wifi_password));
    free(app.log);SDL_DestroyMutex(app.mutex);nk_input_end(ctx);nk_sdl_shutdown(ctx);SDL_DestroyRenderer(renderer);
    SDL_DestroyWindow(window);SDL_Quit();return 0;
}
