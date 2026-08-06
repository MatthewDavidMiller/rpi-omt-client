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
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <direct.h>
#define working_directory(buffer, size) _getcwd((buffer), (int)(size))
#else
#include <unistd.h>
#define working_directory(buffer, size) getcwd((buffer), (size))
#endif

/* The window is laid out in design units and drawn in device pixels. One design
   unit is one pixel at 100% desktop scaling; every metric below is multiplied by
   the window's display scale, so the same layout is legible on a 1366x768 laptop
   panel, a 150% scaled 1080p desktop, and a 4K monitor. */
#define UI_DESIGN_WIDTH 1180.0F
#define UI_DESIGN_HEIGHT 820.0F
#define UI_MIN_WIDTH 620.0F
#define UI_MIN_HEIGHT 520.0F
#define UI_BODY_FONT 15.0F
#define UI_HEADING_FONT 22.0F
#define UI_MONO_FONT 13.0F
#define UI_ACTIVITY_LINES 600U
#define UI_NOTICE_MILLISECONDS 2500ULL
#define UI_CHECK_MILLISECONDS 250ULL
#define UI_ACTIVITY_ID "activity"
#define UI_HSCROLL_STYLE_ITEMS 6

enum operation {
    OP_NONE, OP_TEST, OP_PREREQUISITES, OP_DEPLOY, OP_STATUS, OP_LOGS, OP_RESTART, OP_WIFI
};

enum ui_tab { UI_TAB_CONNECTION, UI_TAB_DEPLOYMENT, UI_TAB_WIFI, UI_TAB_ABOUT, UI_TAB_COUNT };

typedef struct {
    struct nk_color canvas, surface, raised, field, border, divider;
    struct nk_color text, muted, faint;
    struct nk_color accent, accent_bright;
    struct nk_color danger, success, warning;
} ui_theme;

/* A single dark palette with WCAG-AA contrast for body text on every surface. */
static const ui_theme ui_palette = {
    .canvas = {15U, 23U, 42U, 255U},
    .surface = {23U, 33U, 54U, 255U},
    .raised = {30U, 41U, 59U, 255U},
    .field = {12U, 19U, 35U, 255U},
    .border = {51U, 65U, 85U, 255U},
    .divider = {38U, 50U, 71U, 255U},
    .text = {226U, 232U, 240U, 255U},
    .muted = {148U, 163U, 184U, 255U},
    .faint = {100U, 116U, 139U, 255U},
    .accent = {56U, 189U, 248U, 255U},
    .accent_bright = {125U, 211U, 252U, 255U},
    .danger = {248U, 113U, 113U, 255U},
    .success = {74U, 222U, 128U, 255U},
    .warning = {251U, 191U, 36U, 255U}
};

typedef struct {
    struct nk_font_atlas atlas;
    SDL_Texture *texture;
    struct nk_font *body, *heading, *mono;
    float scale;
} ui_fonts;

typedef struct {
    float scale;      /* device pixels per design unit */
    float line;       /* one line of body text */
    float row;        /* a text input or check box */
    float button;
    float heading;
    float mono_line;
    float gap;
    float label_column;
    float reveal_column;   /* zero unless the tab has secrets to align around */
    bool stacked;     /* narrow: field labels sit above their inputs */
    bool columns;     /* wide: the form and the activity log sit side by side */
} ui_metrics;

typedef struct {
    bool valid;
    char message[OMT_DEPLOYER_ERROR_SIZE];
} ui_check;

typedef struct {
    const char *text;
    int length;
} ui_line;

typedef struct {
    char host[256],username[65],port[6],password[512],key_path[1024],key_passphrase[512];
    char sudo_password[512],project_root[2048],remote_directory[256],wifi_ssid[64],wifi_password[128];
    bool use_key,build_image,wifi_connect;
    bool show_password,show_passphrase,show_sudo,show_wifi_password;
    SDL_Thread *thread;
    SDL_Mutex *mutex;
    SDL_AtomicInt cancel;
    SDL_AtomicInt working;
    enum operation operation;
    char *log;
    size_t log_size;
    unsigned log_revision;   /* guarded by mutex; bumped on every append */
    unsigned log_followed;   /* the revision the activity pane last scrolled to */
    float log_extent;        /* previous frame's maximum scroll offset */
    uint64_t notice_until;
    char notice[64];
    ui_check connection_check,deploy_check,wifi_check;
    uint64_t checked_at;
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
    ++app->log_revision;
    SDL_UnlockMutex(app->mutex);
}
#define APPEND_LITERAL(app, text) append_log((app),(text),sizeof(text)-1U)

static void event_log(const char *text,size_t size,void *context){append_log(context,text,size);}
static bool cancelled(void *context){return SDL_GetAtomicInt(&((application *)context)->cancel)!=0;}

static void notice(application *app,const char *text) {
    (void)snprintf(app->notice,sizeof(app->notice),"%s",text);
    app->notice_until=SDL_GetTicks()+UI_NOTICE_MILLISECONDS;
}

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

static const char *operation_label(enum operation operation) {
    switch(operation){
        case OP_TEST:return "Testing connection";
        case OP_PREREQUISITES:return "Installing prerequisites";
        case OP_DEPLOY:return "Building and deploying";
        case OP_STATUS:return "Reading service status";
        case OP_LOGS:return "Fetching recent logs";
        case OP_RESTART:return "Restarting the service";
        case OP_WIFI:return "Applying Wi-Fi settings";
        case OP_NONE:break;
    }
    return "Working";
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

/* The three validators that the deployment service itself enforces are the only
   source of truth for what a tab needs before its actions can run. Re-running
   them a few times a second keeps the buttons and their hints honest without
   duplicating any rule, and without stat()ing the key file every frame. */
static void refresh_checks(application *app) {
    omt_connection connection=connection_of(app);
    omt_deploy_options options=options_of(app);
    omt_wifi_settings wifi={app->wifi_ssid,app->wifi_password,app->wifi_connect};
    app->connection_check.message[0]='\0';app->deploy_check.message[0]='\0';app->wifi_check.message[0]='\0';
    app->connection_check.valid=omt_connection_validate(&connection,app->connection_check.message,
        sizeof(app->connection_check.message));
    app->deploy_check.valid=omt_options_validate(&options,true,app->deploy_check.message,
        sizeof(app->deploy_check.message));
    app->wifi_check.valid=omt_wifi_validate(&wifi,app->wifi_check.message,sizeof(app->wifi_check.message));
}

/* ------------------------------------------------------------------ fonts */

/* Text is baked from a system face at the exact pixel height the display needs,
   so it stays crisp at any scaling factor. Nuklear's built-in bitmap font is the
   fallback when a host ships none of these. */
#ifdef _WIN32
static const char *const ui_body_faces[]={"segoeui.ttf","tahoma.ttf","arial.ttf"};
static const char *const ui_heading_faces[]={"segoeuisb.ttf","segoeuib.ttf","tahomabd.ttf","arialbd.ttf"};
static const char *const ui_mono_faces[]={"consola.ttf","lucon.ttf","cour.ttf"};
#elif defined(__APPLE__)
static const char *const ui_body_faces[]={"/System/Library/Fonts/SFNS.ttf","/Library/Fonts/Arial.ttf"};
static const char *const ui_heading_faces[]={"/System/Library/Fonts/SFNS.ttf","/Library/Fonts/Arial Bold.ttf"};
static const char *const ui_mono_faces[]={"/System/Library/Fonts/Menlo.ttc","/System/Library/Fonts/Courier.ttc"};
#else
static const char *const ui_body_faces[]={
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/redhat-vf/RedHatText[wght].ttf",
    "/usr/share/fonts/google-droid-sans-fonts/DroidSans.ttf",
    "/usr/share/fonts/gnu-free/FreeSans.ttf",
    "/usr/share/fonts/urw-base35/NimbusSans-Regular.otf"};
static const char *const ui_heading_faces[]={
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
    "/usr/share/fonts/google-droid-sans-fonts/DroidSans-Bold.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    "/usr/share/fonts/gnu-free/FreeSansBold.ttf",
    "/usr/share/fonts/urw-base35/NimbusSans-Bold.otf",
    "/usr/share/fonts/google-noto-vf/NotoSans[wght].ttf",
    "/usr/share/fonts/redhat-vf/RedHatText[wght].ttf"};
static const char *const ui_mono_faces[]={
    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
    "/usr/share/fonts/google-noto-vf/NotoSansMono[wght].ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
    "/usr/share/fonts/redhat-vf/RedHatMono[wght].ttf",
    "/usr/share/fonts/gnu-free/FreeMono.ttf",
    "/usr/share/fonts/urw-base35/NimbusMonoPS-Regular.otf"};
#endif

static bool resolve_face(const char *name,char *path,size_t size) {
#ifdef _WIN32
    /* GetWindowsDirectoryA rather than an environment variable, so the search
       path for a parsed font file cannot be redirected by the process env. */
    char directory[MAX_PATH];
    const UINT length=GetWindowsDirectoryA(directory,(UINT)sizeof(directory));
    if(length==0U||(size_t)length>=sizeof(directory))return false;
    return (size_t)snprintf(path,size,"%s\\Fonts\\%s",directory,name)<size;
#else
    return (size_t)snprintf(path,size,"%s",name)<size;
#endif
}

static struct nk_font *add_face(struct nk_font_atlas *atlas,const char *const *faces,size_t count,
                                float height,bool system_faces) {
    if(system_faces){
        for(size_t i=0U;i<count;++i){
            char path[1024];struct nk_font *font;
            if(!resolve_face(faces[i],path,sizeof(path)))continue;
            font=nk_font_atlas_add_from_file(atlas,path,height,NULL);
            if(font!=NULL)return font;
        }
    }
    return nk_font_atlas_add_default(atlas,height,NULL);
}

static void release_fonts(ui_fonts *fonts) {
    if(fonts->texture!=NULL){SDL_DestroyTexture(fonts->texture);fonts->texture=NULL;}
    if(fonts->atlas.permanent.alloc!=NULL)nk_font_atlas_clear(&fonts->atlas);
    memset(fonts,0,sizeof(*fonts));
}

static bool bake_atlas(ui_fonts *built,SDL_Renderer *renderer,float scale,bool system_faces) {
    struct nk_allocator allocator=nk_sdl_allocator();
    const void *image;int width=0,height=0;
    memset(built,0,sizeof(*built));
    nk_font_atlas_init(&built->atlas,&allocator);
    nk_font_atlas_begin(&built->atlas);
    built->body=add_face(&built->atlas,ui_body_faces,NK_LEN(ui_body_faces),
                         SDL_roundf(UI_BODY_FONT*scale),system_faces);
    built->heading=add_face(&built->atlas,ui_heading_faces,NK_LEN(ui_heading_faces),
                            SDL_roundf(UI_HEADING_FONT*scale),system_faces);
    built->mono=add_face(&built->atlas,ui_mono_faces,NK_LEN(ui_mono_faces),
                         SDL_roundf(UI_MONO_FONT*scale),system_faces);
    image=nk_font_atlas_bake(&built->atlas,&width,&height,NK_FONT_ATLAS_RGBA32);
    if(image!=NULL&&built->body!=NULL&&built->heading!=NULL&&built->mono!=NULL&&width>0&&height>0)
        built->texture=SDL_CreateTexture(renderer,SDL_PIXELFORMAT_ARGB8888,SDL_TEXTUREACCESS_STATIC,
                                         width,height);
    if(built->texture==NULL){
        nk_font_atlas_end(&built->atlas,nk_handle_ptr(NULL),NULL);
        nk_font_atlas_clear(&built->atlas);
        memset(built,0,sizeof(*built));
        return false;
    }
    (void)SDL_UpdateTexture(built->texture,NULL,image,width*4);
    (void)SDL_SetTextureBlendMode(built->texture,SDL_BLENDMODE_BLEND);
    nk_font_atlas_end(&built->atlas,nk_handle_ptr(built->texture),NULL);
    nk_font_atlas_cleanup(&built->atlas);
    built->scale=scale;
    return true;
}

/* A system face that stb_truetype cannot parse fails the whole atlas, so the
   built-in font is retried before the interface is declared unusable. */
static bool build_fonts(ui_fonts *fonts,SDL_Renderer *renderer,struct nk_context *ctx,float scale) {
    ui_fonts built;
    if(!bake_atlas(&built,renderer,scale,true)&&!bake_atlas(&built,renderer,scale,false))return false;
    release_fonts(fonts);
    *fonts=built;
    nk_style_set_font(ctx,&fonts->body->handle);
    return true;
}

/* ------------------------------------------------------------------ style */

static void apply_style(struct nk_context *ctx,float scale) {
    struct nk_color table[NK_COLOR_COUNT];
    struct nk_style *style;
    table[NK_COLOR_TEXT]=ui_palette.text;
    table[NK_COLOR_WINDOW]=ui_palette.canvas;
    table[NK_COLOR_HEADER]=ui_palette.raised;
    table[NK_COLOR_BORDER]=ui_palette.border;
    table[NK_COLOR_BUTTON]=ui_palette.raised;
    table[NK_COLOR_BUTTON_HOVER]=ui_palette.border;
    table[NK_COLOR_BUTTON_ACTIVE]=ui_palette.divider;
    table[NK_COLOR_TOGGLE]=ui_palette.field;
    table[NK_COLOR_TOGGLE_HOVER]=ui_palette.raised;
    table[NK_COLOR_TOGGLE_CURSOR]=ui_palette.accent;
    table[NK_COLOR_SELECT]=ui_palette.raised;
    table[NK_COLOR_SELECT_ACTIVE]=ui_palette.accent;
    table[NK_COLOR_SLIDER]=ui_palette.field;
    table[NK_COLOR_SLIDER_CURSOR]=ui_palette.accent;
    table[NK_COLOR_SLIDER_CURSOR_HOVER]=ui_palette.accent_bright;
    table[NK_COLOR_SLIDER_CURSOR_ACTIVE]=ui_palette.accent_bright;
    table[NK_COLOR_PROPERTY]=ui_palette.field;
    table[NK_COLOR_EDIT]=ui_palette.field;
    table[NK_COLOR_EDIT_CURSOR]=ui_palette.accent;
    table[NK_COLOR_COMBO]=ui_palette.raised;
    table[NK_COLOR_CHART]=ui_palette.raised;
    table[NK_COLOR_CHART_COLOR]=ui_palette.accent;
    table[NK_COLOR_CHART_COLOR_HIGHLIGHT]=ui_palette.danger;
    table[NK_COLOR_SCROLLBAR]=ui_palette.canvas;
    table[NK_COLOR_SCROLLBAR_CURSOR]=ui_palette.border;
    table[NK_COLOR_SCROLLBAR_CURSOR_HOVER]=ui_palette.faint;
    table[NK_COLOR_SCROLLBAR_CURSOR_ACTIVE]=ui_palette.muted;
    table[NK_COLOR_TAB_HEADER]=ui_palette.raised;
    table[NK_COLOR_KNOB]=ui_palette.field;
    table[NK_COLOR_KNOB_CURSOR]=ui_palette.accent;
    table[NK_COLOR_KNOB_CURSOR_HOVER]=ui_palette.accent_bright;
    table[NK_COLOR_KNOB_CURSOR_ACTIVE]=ui_palette.accent_bright;
    nk_style_from_table(ctx,table);

    style=&ctx->style;
    /* Nuklear otherwise inflates every row to the font height plus 16 pixels,
       which would override the scaled metrics computed below. */
    style->window.min_row_height_padding=SDL_roundf(scale);
    style->window.background=ui_palette.canvas;
    style->window.fixed_background=nk_style_item_color(ui_palette.canvas);
    style->window.border=0.0F;
    style->window.rounding=SDL_roundf(10.0F*scale);
    style->window.padding=nk_vec2(SDL_roundf(20.0F*scale),SDL_roundf(18.0F*scale));
    style->window.group_padding=nk_vec2(SDL_roundf(16.0F*scale),SDL_roundf(14.0F*scale));
    style->window.spacing=nk_vec2(SDL_roundf(10.0F*scale),SDL_roundf(8.0F*scale));
    style->window.scrollbar_size=nk_vec2(SDL_roundf(12.0F*scale),SDL_roundf(12.0F*scale));
    style->window.group_border=SDL_roundf(scale);
    style->window.group_border_color=ui_palette.divider;
    style->window.header.normal=nk_style_item_color(ui_palette.raised);
    style->window.header.hover=nk_style_item_color(ui_palette.raised);
    style->window.header.active=nk_style_item_color(ui_palette.raised);
    style->window.header.label_normal=ui_palette.muted;
    style->window.header.label_hover=ui_palette.muted;
    style->window.header.label_active=ui_palette.muted;
    style->window.header.padding=nk_vec2(SDL_roundf(14.0F*scale),SDL_roundf(8.0F*scale));
    style->window.header.label_padding=nk_vec2(SDL_roundf(2.0F*scale),SDL_roundf(2.0F*scale));

    style->button.rounding=SDL_roundf(6.0F*scale);
    style->button.border=SDL_roundf(scale);
    style->button.border_color=ui_palette.border;
    style->button.padding=nk_vec2(SDL_roundf(12.0F*scale),SDL_roundf(4.0F*scale));
    style->button.text_normal=ui_palette.text;
    style->button.text_hover=ui_palette.text;
    style->button.text_active=ui_palette.accent_bright;

    style->edit.rounding=SDL_roundf(6.0F*scale);
    style->edit.border=SDL_roundf(scale);
    style->edit.border_color=ui_palette.border;
    style->edit.padding=nk_vec2(SDL_roundf(10.0F*scale),SDL_roundf(4.0F*scale));
    style->edit.cursor_size=SDL_roundf(2.0F*scale);
    style->edit.hover=nk_style_item_color(ui_palette.field);
    style->edit.active=nk_style_item_color(ui_palette.field);
    style->edit.selected_normal=ui_palette.accent;
    style->edit.selected_hover=ui_palette.accent;
    style->edit.selected_text_normal=ui_palette.canvas;
    style->edit.selected_text_hover=ui_palette.canvas;

    style->checkbox.border=SDL_roundf(scale);
    style->checkbox.border_color=ui_palette.border;
    style->checkbox.padding=nk_vec2(SDL_roundf(3.0F*scale),SDL_roundf(3.0F*scale));
    style->checkbox.spacing=SDL_roundf(10.0F*scale);
    style->checkbox.text_normal=ui_palette.text;
    style->checkbox.text_hover=ui_palette.text;
    style->checkbox.text_active=ui_palette.text;

    style->scrollv.rounding=SDL_roundf(6.0F*scale);
    style->scrollv.rounding_cursor=SDL_roundf(6.0F*scale);
    style->scrollh.rounding=SDL_roundf(6.0F*scale);
    style->scrollh.rounding_cursor=SDL_roundf(6.0F*scale);
    style->scrollv.show_buttons=nk_false;
    style->scrollh.show_buttons=nk_false;
}

/* ------------------------------------------------------------- primitives */

static float clampf(float value,float low,float high) {
    return value<low?low:(value>high?high:value);
}

static float text_width(const struct nk_user_font *font,const char *text) {
    return font->width(font->userdata,font->height,text,(int)strlen(text));
}

/* Nuklear insets button text by the padding, the border, and the rounding, so a
   self-sizing button has to reserve all three or it truncates its own label. */
static float button_width(const struct nk_context *ctx,const struct nk_user_font *font,const char *label) {
    const struct nk_style_button *style=&ctx->style.button;
    return text_width(font,label)+2.0F*(style->padding.x+style->border+style->rounding)+2.0F;
}

static struct nk_rect thin_row(struct nk_context *ctx,float thickness) {
    struct nk_rect bounds;
    nk_layout_set_min_row_height(ctx,thickness);
    nk_layout_row_dynamic(ctx,thickness,1);
    bounds=nk_widget_bounds(ctx);
    nk_spacer(ctx);
    nk_layout_reset_min_row_height(ctx);
    return bounds;
}

static void divider(struct nk_context *ctx,const ui_metrics *metrics) {
    const struct nk_rect bounds=thin_row(ctx,SDL_roundf(metrics->scale));
    nk_fill_rect(nk_window_get_canvas(ctx),bounds,0.0F,ui_palette.divider);
}

/* An indeterminate progress indicator: the deployment service reports discrete
   steps rather than a percentage, so the bar communicates liveness only. */
static void progress_bar(struct nk_context *ctx,const ui_metrics *metrics,bool working,uint64_t ticks) {
    const struct nk_rect track=thin_row(ctx,SDL_roundf(3.0F*metrics->scale));
    struct nk_command_buffer *canvas=nk_window_get_canvas(ctx);
    nk_fill_rect(canvas,track,track.h*0.5F,ui_palette.divider);
    if(working&&track.w>0.0F){
        const float span=track.w*0.28F;
        const float phase=(float)(ticks%1500ULL)/1500.0F;
        struct nk_rect cursor=track;
        cursor.x=track.x+phase*(track.w+span)-span;
        cursor.w=span;
        if(cursor.x<track.x){cursor.w-=track.x-cursor.x;cursor.x=track.x;}
        if(cursor.x+cursor.w>track.x+track.w)cursor.w=track.x+track.w-cursor.x;
        if(cursor.w>0.0F)nk_fill_rect(canvas,cursor,cursor.h*0.5F,ui_palette.accent);
    }
}

static bool action_button(struct nk_context *ctx,const char *label,bool enabled,bool primary,
                          struct nk_color tint) {
    struct nk_style_button style=ctx->style.button;
    bool clicked;
    if(primary){
        style.normal=nk_style_item_color(tint);
        style.hover=nk_style_item_color(ui_palette.accent_bright);
        style.active=nk_style_item_color(tint);
        style.border_color=tint;
        style.text_normal=ui_palette.canvas;
        style.text_hover=ui_palette.canvas;
        style.text_active=ui_palette.canvas;
    } else {
        style.text_normal=tint;
        style.text_hover=tint;
        style.text_active=tint;
    }
    if(!enabled)nk_widget_disable_begin(ctx);
    clicked=nk_button_label_styled(ctx,&style,label)!=0;
    if(!enabled)nk_widget_disable_end(ctx);
    return clicked&&enabled;
}

static bool tab_button(struct nk_context *ctx,const ui_metrics *metrics,const char *label,bool selected) {
    struct nk_style_button style=ctx->style.button;
    style.rounding=SDL_roundf(6.0F*metrics->scale);
    if(selected){
        style.normal=nk_style_item_color(ui_palette.raised);
        style.hover=nk_style_item_color(ui_palette.raised);
        style.active=nk_style_item_color(ui_palette.raised);
        style.border_color=ui_palette.accent;
        style.text_normal=ui_palette.accent;
        style.text_hover=ui_palette.accent;
        style.text_active=ui_palette.accent;
    } else {
        style.normal=nk_style_item_color(ui_palette.canvas);
        style.hover=nk_style_item_color(ui_palette.surface);
        style.active=nk_style_item_color(ui_palette.surface);
        style.border_color=ui_palette.divider;
        style.text_normal=ui_palette.muted;
        style.text_hover=ui_palette.text;
        style.text_active=ui_palette.text;
    }
    return nk_button_label_styled(ctx,&style,label)!=0;
}

static void heading(struct nk_context *ctx,const ui_metrics *metrics,const char *title) {
    nk_layout_row_dynamic(ctx,metrics->line,1);
    nk_label_colored(ctx,title,NK_TEXT_LEFT,ui_palette.accent);
    divider(ctx,metrics);
}

static void note(struct nk_context *ctx,const ui_metrics *metrics,const char *text,int lines,
                 struct nk_color color) {
    nk_layout_row_dynamic(ctx,metrics->line*(float)lines,1);
    nk_label_colored_wrap(ctx,text,color);
}

static void spacer_row(struct nk_context *ctx,const ui_metrics *metrics) {
    nk_layout_set_min_row_height(ctx,metrics->gap);
    nk_layout_row_dynamic(ctx,metrics->gap,1);
    nk_spacer(ctx);
    nk_layout_reset_min_row_height(ctx);
}

/* A labelled text input. Wide windows put the caption in a fixed left column so
   the inputs line up; narrow windows stack the caption above its input. Secret
   fields get a reveal toggle rather than being permanently unreadable. */
static void field(struct nk_context *ctx,const ui_metrics *metrics,const char *label,char *value,
                  int capacity,nk_plugin_filter filter,bool *reveal,bool enabled,const char *hint) {
    struct nk_style_edit saved=ctx->style.edit;
    const bool masked=reveal!=NULL&&!*reveal;
    if(metrics->stacked){
        nk_layout_row_dynamic(ctx,metrics->line,1);
        nk_label_colored(ctx,label,NK_TEXT_LEFT,ui_palette.muted);
        nk_layout_row_template_begin(ctx,metrics->row);
        nk_layout_row_template_push_dynamic(ctx);
        if(metrics->reveal_column>0.0F)nk_layout_row_template_push_static(ctx,metrics->reveal_column);
        nk_layout_row_template_end(ctx);
    } else {
        nk_layout_row_template_begin(ctx,metrics->row);
        nk_layout_row_template_push_static(ctx,metrics->label_column);
        nk_layout_row_template_push_dynamic(ctx);
        if(metrics->reveal_column>0.0F)nk_layout_row_template_push_static(ctx,metrics->reveal_column);
        nk_layout_row_template_end(ctx);
        nk_label_colored(ctx,label,NK_TEXT_LEFT,ui_palette.muted);
    }
    if(masked){
        ctx->style.edit.text_normal.a=0U;ctx->style.edit.text_hover.a=0U;ctx->style.edit.text_active.a=0U;
        ctx->style.edit.selected_text_normal.a=0U;ctx->style.edit.selected_text_hover.a=0U;
        ctx->style.edit.cursor_text_normal.a=0U;ctx->style.edit.cursor_text_hover.a=0U;
    }
    if(!enabled)nk_widget_disable_begin(ctx);
    (void)nk_edit_string_zero_terminated(ctx,NK_EDIT_FIELD,value,capacity,filter);
    if(!enabled)nk_widget_disable_end(ctx);
    if(masked)ctx->style.edit=saved;
    /* Inputs on a tab that has any secret all reserve the reveal column, so
       every field in the form still ends on the same edge. */
    if(metrics->reveal_column>0.0F){
        if(reveal==NULL)nk_spacer(ctx);
        else if(nk_button_label(ctx,*reveal?"Hide":"Show"))*reveal=!*reveal;
    }
    if(hint!=NULL){
        nk_layout_row_template_begin(ctx,metrics->line);
        if(!metrics->stacked)nk_layout_row_template_push_static(ctx,metrics->label_column);
        nk_layout_row_template_push_dynamic(ctx);
        nk_layout_row_template_end(ctx);
        if(!metrics->stacked)nk_spacer(ctx);
        nk_label_colored(ctx,hint,NK_TEXT_LEFT,ui_palette.faint);
    }
}

/* Nuklear widens the last column of a row by the rounding error of the columns
   before it, which can push a form a fraction of a pixel past its panel and
   raise a horizontal scrollbar for content that never scrolls sideways. */
static void push_hidden_hscroll(struct nk_context *ctx) {
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.normal,nk_style_item_hide());
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.hover,nk_style_item_hide());
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.active,nk_style_item_hide());
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.cursor_normal,nk_style_item_hide());
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.cursor_hover,nk_style_item_hide());
    (void)nk_style_push_style_item(ctx,&ctx->style.scrollh.cursor_active,nk_style_item_hide());
}

static void pop_hidden_hscroll(struct nk_context *ctx) {
    for(int index=0;index<UI_HSCROLL_STYLE_ITEMS;++index)(void)nk_style_pop_style_item(ctx);
}

static void check_box(struct nk_context *ctx,const ui_metrics *metrics,const char *label,bool *value,
                      bool enabled) {
    nk_layout_row_dynamic(ctx,metrics->row,1);
    if(!enabled)nk_widget_disable_begin(ctx);
    *value=nk_check_label(ctx,label,*value?1:0)!=0;
    if(!enabled)nk_widget_disable_end(ctx);
}

/* The live validation message for a tab, shown next to the actions it gates. */
static void requirement(struct nk_context *ctx,const ui_metrics *metrics,const ui_check *check) {
    if(check->valid||check->message[0]=='\0')return;
    note(ctx,metrics,check->message,2,ui_palette.warning);
}

/* --------------------------------------------------------- monospace text */

/* Every log and legal-text pane renders one widget per source line at a fixed
   row width. The monospace face makes the widest line exactly computable, so the
   panes scroll in both axes instead of guessing a wrapped height. */
static size_t tail_offset(const char *text,size_t size,size_t max_lines) {
    size_t seen=0U,index=size;
    while(index>0U){
        --index;
        if(text[index]!='\n'||index+1U==size)continue;
        if(++seen>=max_lines)return index+1U;
    }
    return 0U;
}

static size_t split_lines(const char *text,size_t size,ui_line *lines,size_t capacity) {
    size_t count=0U,start=0U;
    for(size_t index=0U;index<=size&&count<capacity;++index){
        if(index!=size&&text[index]!='\n')continue;
        if(index==size&&index==start)break;
        lines[count].text=text+start;
        lines[count].length=(int)(index-start);
        ++count;start=index+1U;
    }
    return count;
}

static struct nk_color line_color(const ui_line *line) {
    if(line->length>=5&&memcmp(line->text,"ERROR",5U)==0)return ui_palette.danger;
    if(line->length>=3&&memcmp(line->text,"---",3U)==0)return ui_palette.accent;
    if(line->length>=5&&memcmp(line->text,"=====",5U)==0)return ui_palette.accent;
    if(line->length>=19&&memcmp(line->text,"Operation completed",19U)==0)return ui_palette.success;
    return ui_palette.text;
}

static float draw_lines(struct nk_context *ctx,const ui_metrics *metrics,const ui_fonts *fonts,
                        const ui_line *lines,size_t count) {
    const struct nk_user_font *font=&fonts->mono->handle;
    const float advance=text_width(font,"0");
    float height;
    int widest=0;
    for(size_t index=0U;index<count;++index){
        const int glyphs=nk_utf_len(lines[index].text,lines[index].length);
        if(glyphs>widest)widest=glyphs;
    }
    /* Command output and licence text read as prose, not as a stack of separate
       controls, so these rows sit on a tighter leading than the rest of the UI. */
    (void)nk_style_push_vec2(ctx,&ctx->style.window.spacing,
                             nk_vec2(ctx->style.window.spacing.x,SDL_roundf(2.0F*metrics->scale)));
    nk_layout_row_static(ctx,metrics->mono_line,(int)(((float)widest+1.0F)*advance),1);
    for(size_t index=0U;index<count;++index)
        nk_text_colored(ctx,lines[index].text,lines[index].length,NK_TEXT_LEFT,line_color(&lines[index]));
    height=(float)count*(metrics->mono_line+ctx->style.window.spacing.y);
    (void)nk_style_pop_vec2(ctx);
    return height;
}

/* ------------------------------------------------------------------- tabs */

static void draw_connection(struct nk_context *ctx,application *app,const ui_metrics *base,bool idle) {
    ui_metrics scoped=*base;const ui_metrics *metrics=&scoped;
    scoped.reveal_column=SDL_roundf(70.0F*scoped.scale);
    heading(ctx,metrics,"RASPBERRY PI");
    field(ctx,metrics,"Host",app->host,(int)sizeof(app->host),nk_filter_default,NULL,idle,
          "IPv4 address or DNS name of the Pi.");
    field(ctx,metrics,"SSH user",app->username,(int)sizeof(app->username),nk_filter_default,NULL,idle,NULL);
    field(ctx,metrics,"SSH port",app->port,(int)sizeof(app->port),nk_filter_decimal,NULL,idle,
          "Defaults to 22.");
    heading(ctx,metrics,"AUTHENTICATION");
    check_box(ctx,metrics,"Authenticate with a private key",&app->use_key,idle);
    if(app->use_key){
        field(ctx,metrics,"Private key",app->key_path,(int)sizeof(app->key_path),nk_filter_default,NULL,idle,
              "Full path to the key file on this machine.");
        field(ctx,metrics,"Key passphrase",app->key_passphrase,(int)sizeof(app->key_passphrase),
              nk_filter_default,&app->show_passphrase,idle,NULL);
    } else {
        field(ctx,metrics,"SSH password",app->password,(int)sizeof(app->password),nk_filter_default,
              &app->show_password,idle,NULL);
    }
    field(ctx,metrics,"Sudo password",app->sudo_password,(int)sizeof(app->sudo_password),nk_filter_default,
          &app->show_sudo,idle,"Optional. Leave empty for passwordless sudo.");
    spacer_row(ctx,metrics);
    nk_layout_row_dynamic(ctx,metrics->button,1);
    if(action_button(ctx,"Test connection",idle&&app->connection_check.valid,true,ui_palette.accent))
        start(app,OP_TEST);
    requirement(ctx,metrics,&app->connection_check);
    note(ctx,metrics,"Host keys must already exist in ~/.ssh/known_hosts. Unknown or changed keys are refused.",
         3,ui_palette.muted);
}

static void draw_deployment(struct nk_context *ctx,application *app,const ui_metrics *metrics,bool idle) {
    const bool ready=idle&&app->connection_check.valid&&app->deploy_check.valid;
    heading(ctx,metrics,"SOURCES");
    field(ctx,metrics,"Project root",app->project_root,(int)sizeof(app->project_root),nk_filter_default,NULL,
          idle,"Detected automatically when the deployer runs inside a checkout.");
    field(ctx,metrics,"Remote directory",app->remote_directory,(int)sizeof(app->remote_directory),
          nk_filter_default,NULL,idle,"Absolute install path on the Pi.");
    check_box(ctx,metrics,"Build the ARM64 image before deploying",&app->build_image,idle);
    spacer_row(ctx,metrics);
    heading(ctx,metrics,"DEPLOY");
    nk_layout_row_dynamic(ctx,metrics->button,metrics->stacked?1:2);
    if(action_button(ctx,"Build and deploy",ready,true,ui_palette.accent))start(app,OP_DEPLOY);
    if(action_button(ctx,"Install prerequisites",idle&&app->project_root[0]!='\0',false,ui_palette.text))
        start(app,OP_PREREQUISITES);
    requirement(ctx,metrics,&app->deploy_check);
    spacer_row(ctx,metrics);
    heading(ctx,metrics,"MANAGE THE RUNNING SERVICE");
    nk_layout_row_dynamic(ctx,metrics->button,metrics->stacked?1:3);
    if(action_button(ctx,"Status",idle&&app->connection_check.valid,false,ui_palette.text))start(app,OP_STATUS);
    if(action_button(ctx,"Recent logs",idle&&app->connection_check.valid,false,ui_palette.text))start(app,OP_LOGS);
    if(action_button(ctx,"Restart",idle&&app->connection_check.valid,false,ui_palette.text))start(app,OP_RESTART);
    requirement(ctx,metrics,&app->connection_check);
}

static void draw_wifi(struct nk_context *ctx,application *app,const ui_metrics *base,bool idle) {
    ui_metrics scoped=*base;const ui_metrics *metrics=&scoped;
    scoped.reveal_column=SDL_roundf(70.0F*scoped.scale);
    heading(ctx,metrics,"WIRELESS NETWORK");
    field(ctx,metrics,"SSID",app->wifi_ssid,(int)sizeof(app->wifi_ssid),nk_filter_default,NULL,idle,NULL);
    field(ctx,metrics,"Password",app->wifi_password,(int)sizeof(app->wifi_password),nk_filter_default,
          &app->show_wifi_password,idle,"8-63 characters, or a 64-digit hex PSK.");
    check_box(ctx,metrics,"Switch to this network immediately",&app->wifi_connect,idle);
    spacer_row(ctx,metrics);
    nk_layout_row_dynamic(ctx,metrics->button,1);
    if(action_button(ctx,"Apply Wi-Fi settings",idle&&app->connection_check.valid&&app->wifi_check.valid,
                     true,ui_palette.accent))start(app,OP_WIFI);
    requirement(ctx,metrics,&app->wifi_check);
    requirement(ctx,metrics,&app->connection_check);
    note(ctx,metrics,"Connecting can interrupt this SSH session if the Pi changes networks.",2,
         ui_palette.warning);
}

static void draw_about(struct nk_context *ctx,const ui_metrics *metrics,const ui_fonts *fonts) {
    const omt_legal_document *documents=NULL;
    const size_t count=omt_legal_documents(&documents);
    static ui_line lines[UI_ACTIVITY_LINES];
    char text[256];
    nk_layout_row_dynamic(ctx,metrics->heading,1);
    (void)nk_style_push_font(ctx,&fonts->heading->handle);
    nk_label_colored(ctx,"Raspberry Pi OMT Deployer",NK_TEXT_LEFT,ui_palette.text);
    (void)nk_style_pop_font(ctx);
    (void)snprintf(text,sizeof(text),"Version %s",OMT_CLIENT_VERSION);
    nk_layout_row_dynamic(ctx,metrics->line,1);
    nk_label_colored(ctx,text,NK_TEXT_LEFT,ui_palette.muted);
    nk_label_colored(ctx,"Copyright (c) 2026 Matthew David Miller",NK_TEXT_LEFT,ui_palette.muted);
    note(ctx,metrics,"Native C17 deployer built on SDL3, Nuklear, and libssh2. Project code is MIT licensed.",
         3,ui_palette.muted);
    for(size_t index=0U;index<count;++index){
        size_t used;
        (void)snprintf(text,sizeof(text),"===== %s =====",documents[index].name);
        spacer_row(ctx,metrics);
        nk_layout_row_dynamic(ctx,metrics->line,1);
        nk_label_colored(ctx,text,NK_TEXT_LEFT,ui_palette.accent);
        (void)nk_style_push_font(ctx,&fonts->mono->handle);
        used=split_lines(documents[index].text,documents[index].text_size,lines,NK_LEN(lines));
        (void)draw_lines(ctx,metrics,fonts,lines,used);
        (void)nk_style_pop_font(ctx);
    }
}

/* --------------------------------------------------------------- activity */

static void draw_activity(struct nk_context *ctx,application *app,const ui_metrics *metrics,
                          const ui_fonts *fonts,float pane_height) {
    static ui_line lines[UI_ACTIVITY_LINES];
    nk_uint offset_x=0U,offset_y=0U;
    size_t count;
    float content;

    /* Follow new output only while the operator is already reading the end of
       the log, so scrolling back through a deployment is never interrupted. */
    nk_group_get_scroll(ctx,UI_ACTIVITY_ID,&offset_x,&offset_y);
    SDL_LockMutex(app->mutex);
    if(app->log_revision!=app->log_followed){
        if((float)offset_y+1.5F*metrics->mono_line>=app->log_extent)
            nk_group_set_scroll(ctx,UI_ACTIVITY_ID,offset_x,(nk_uint)1U<<28U);
        app->log_followed=app->log_revision;
    }
    if(app->log==NULL||app->log_size==0U){
        count=0U;
    } else {
        const size_t start=tail_offset(app->log,app->log_size,UI_ACTIVITY_LINES);
        count=split_lines(app->log+start,app->log_size-start,lines,NK_LEN(lines));
    }
    content=0.0F;
    (void)nk_style_push_style_item(ctx,&ctx->style.window.fixed_background,
                                   nk_style_item_color(ui_palette.surface));
    if(nk_group_begin_titled(ctx,UI_ACTIVITY_ID,"ACTIVITY LOG",NK_WINDOW_BORDER|NK_WINDOW_TITLE)){
        (void)nk_style_push_font(ctx,&fonts->mono->handle);
        if(count==0U){
            nk_layout_row_dynamic(ctx,metrics->mono_line,1);
            nk_label_colored(ctx,"Ready.",NK_TEXT_LEFT,ui_palette.muted);
        } else {
            content=draw_lines(ctx,metrics,fonts,lines,count);
        }
        (void)nk_style_pop_font(ctx);
        nk_group_end(ctx);
    }
    (void)nk_style_pop_style_item(ctx);
    SDL_UnlockMutex(app->mutex);
    app->log_extent=clampf(content-(pane_height-metrics->heading-4.0F*metrics->gap),0.0F,1.0e6F);
}

/* ------------------------------------------------------------------ frame */

static ui_metrics measure(const ui_fonts *fonts,float scale,float width,float height) {
    ui_metrics metrics;
    float form_units;
    metrics.scale=scale;
    metrics.line=SDL_roundf(fonts->body->handle.height*1.45F);
    metrics.row=SDL_roundf(fonts->body->handle.height*2.0F);
    metrics.button=SDL_roundf(fonts->body->handle.height*2.3F);
    metrics.heading=SDL_roundf(fonts->heading->handle.height*1.35F);
    metrics.mono_line=SDL_roundf(fonts->mono->handle.height*1.4F);
    metrics.gap=SDL_roundf(8.0F*scale);
    metrics.reveal_column=0.0F;
    metrics.columns=width/scale>=880.0F&&height/scale>=420.0F;
    form_units=(metrics.columns?width*0.56F:width)/scale-80.0F;
    metrics.stacked=form_units<560.0F;
    metrics.label_column=SDL_roundf(clampf(form_units*0.34F,130.0F,210.0F)*scale);
    return metrics;
}

static void draw_header(struct nk_context *ctx,application *app,const ui_metrics *metrics,
                        const ui_fonts *fonts,bool idle,uint64_t now) {
    const struct nk_user_font *font=&fonts->body->handle;
    const char *status;
    struct nk_color status_color;
    char version[128];
    if(now<app->notice_until){status=app->notice;status_color=ui_palette.success;}
    else if(idle){status="Ready";status_color=ui_palette.muted;}
    else{status=operation_label(app->operation);status_color=ui_palette.accent;}
    nk_layout_row_template_begin(ctx,metrics->heading);
    nk_layout_row_template_push_dynamic(ctx);
    nk_layout_row_template_push_static(ctx,clampf(text_width(font,status)+metrics->gap,0.0F,
                                                  metrics->columns?1.0e6F:200.0F*metrics->scale));
    nk_layout_row_template_end(ctx);
    (void)nk_style_push_font(ctx,&fonts->heading->handle);
    nk_label_colored(ctx,"Raspberry Pi OMT Deployer",NK_TEXT_LEFT,ui_palette.text);
    (void)nk_style_pop_font(ctx);
    nk_label_colored(ctx,status,NK_TEXT_RIGHT,status_color);
    (void)snprintf(version,sizeof(version),"Version %s",OMT_CLIENT_VERSION);
    nk_layout_row_dynamic(ctx,metrics->line,1);
    nk_label_colored(ctx,version,NK_TEXT_LEFT,ui_palette.faint);
}

static void draw_tabs(struct nk_context *ctx,application *app,const ui_metrics *metrics) {
    static const char *const labels[UI_TAB_COUNT]={"Connection","Deployment","Wi-Fi","About"};
    nk_layout_row_dynamic(ctx,metrics->button,UI_TAB_COUNT);
    for(int index=0;index<UI_TAB_COUNT;++index)
        if(tab_button(ctx,metrics,labels[index],app->tab==index))app->tab=index;
}

static void draw_form(struct nk_context *ctx,application *app,const ui_metrics *metrics,
                      const ui_fonts *fonts,bool idle) {
    (void)nk_style_push_style_item(ctx,&ctx->style.window.fixed_background,
                                   nk_style_item_color(ui_palette.surface));
    push_hidden_hscroll(ctx);
    if(nk_group_begin(ctx,"form",NK_WINDOW_BORDER)){
        if(app->tab==UI_TAB_CONNECTION)draw_connection(ctx,app,metrics,idle);
        else if(app->tab==UI_TAB_DEPLOYMENT)draw_deployment(ctx,app,metrics,idle);
        else if(app->tab==UI_TAB_WIFI)draw_wifi(ctx,app,metrics,idle);
        else draw_about(ctx,metrics,fonts);
        nk_group_end(ctx);
    }
    pop_hidden_hscroll(ctx);
    (void)nk_style_pop_style_item(ctx);
}

static void draw_footer(struct nk_context *ctx,application *app,const ui_metrics *metrics,
                        const ui_fonts *fonts,bool idle) {
    const struct nk_user_font *font=&fonts->body->handle;
    /* The footer follows the window, not the form pane: only a genuinely narrow
       window has to give up the self-sized buttons and the closing hint. */
    if(!metrics->columns){
        nk_layout_row_dynamic(ctx,metrics->button,3);
    } else {
        nk_layout_row_template_begin(ctx,metrics->button);
        nk_layout_row_template_push_static(ctx,button_width(ctx,font,"Cancel operation"));
        nk_layout_row_template_push_static(ctx,button_width(ctx,font,"Copy log"));
        nk_layout_row_template_push_static(ctx,button_width(ctx,font,"Clear log"));
        nk_layout_row_template_push_dynamic(ctx);
        nk_layout_row_template_end(ctx);
    }
    if(action_button(ctx,"Cancel operation",!idle,false,ui_palette.danger)){
        SDL_SetAtomicInt(&app->cancel,1);
        APPEND_LITERAL(app,"Cancellation requested; waiting for a safe boundary.\n");
    }
    if(action_button(ctx,"Copy log",true,false,ui_palette.text)){
        SDL_LockMutex(app->mutex);
        (void)SDL_SetClipboardText(app->log==NULL?"":app->log);
        SDL_UnlockMutex(app->mutex);
        notice(app,"Activity log copied");
    }
    if(action_button(ctx,"Clear log",idle,false,ui_palette.text)){
        SDL_LockMutex(app->mutex);
        if(app->log!=NULL)app->log[0]='\0';
        app->log_size=0U;++app->log_revision;
        SDL_UnlockMutex(app->mutex);
    }
    if(metrics->columns)
        nk_label_colored(ctx,"Secrets stay in memory and are wiped on exit.",NK_TEXT_RIGHT,ui_palette.faint);
}

static void draw_ui(struct nk_context *ctx,application *app,const ui_metrics *metrics,
                    const ui_fonts *fonts,float width,float height,uint64_t now) {
    const bool idle=SDL_GetAtomicInt(&app->working)==0;
    if(nk_begin(ctx,"deployer",nk_rect(0.0F,0.0F,width,height),NK_WINDOW_NO_SCROLLBAR)){
        const float spacing=ctx->style.window.spacing.y;
        /* The panel's content region spans the whole window; nuklear charges the
           top padding to the first row and never reserves the bottom edge. */
        const float chrome=metrics->heading+metrics->line+SDL_roundf(3.0F*metrics->scale)+
                           2.0F*metrics->button+6.0F*spacing+2.0F*ctx->style.window.padding.y;
        const float body=clampf(nk_window_get_content_region(ctx).h-chrome,metrics->row*3.0F,1.0e6F);
        draw_header(ctx,app,metrics,fonts,idle,now);
        progress_bar(ctx,metrics,!idle,now);
        draw_tabs(ctx,app,metrics);
        if(metrics->columns){
            static const float ratio[2]={0.56F,0.44F};
            nk_layout_row(ctx,NK_DYNAMIC,body,2,ratio);
            draw_form(ctx,app,metrics,fonts,idle);
            draw_activity(ctx,app,metrics,fonts,body);
        } else {
            const float log_height=SDL_roundf(clampf(body*0.34F,
                metrics->heading+4.0F*metrics->mono_line,body*0.5F));
            const float form_height=body-log_height-spacing;
            nk_layout_row_dynamic(ctx,form_height,1);
            draw_form(ctx,app,metrics,fonts,idle);
            nk_layout_row_dynamic(ctx,log_height,1);
            draw_activity(ctx,app,metrics,fonts,log_height);
        }
        draw_footer(ctx,app,metrics,fonts,idle);
    }
    nk_end(ctx);
}

/* ------------------------------------------------------------------- main */

static float window_scale(SDL_Window *window) {
    /* SDL folds the display's content scale and the window's pixel density into
       one factor, which is exactly the multiplier for drawing in pixels. */
    const float scale=SDL_GetWindowDisplayScale(window);
    return scale>0.1F?clampf(scale,0.75F,4.0F):1.0F;
}

/* Window sizes are given in screen coordinates, so a design size in pixels has
   to be divided back out by the window's pixel density. */
static float window_units(SDL_Window *window,float scale) {
    const float density=SDL_GetWindowPixelDensity(window);
    return scale/(density>0.1F?density:1.0F);
}

static void apply_minimum_size(SDL_Window *window,float scale) {
    const float units=window_units(window,scale);
    (void)SDL_SetWindowMinimumSize(window,(int)SDL_roundf(UI_MIN_WIDTH*units),
                                   (int)SDL_roundf(UI_MIN_HEIGHT*units));
}

static void size_for_scale(SDL_Window *window,float scale) {
    const float units=window_units(window,scale);
    SDL_Rect usable={0,0,0,0};
    int width=(int)SDL_roundf(UI_DESIGN_WIDTH*units);
    int height=(int)SDL_roundf(UI_DESIGN_HEIGHT*units);
    apply_minimum_size(window,scale);
    if(SDL_GetDisplayUsableBounds(SDL_GetDisplayForWindow(window),&usable)&&usable.w>0&&usable.h>0){
        if(width>usable.w)width=usable.w;
        if(height>usable.h)height=usable.h;
    }
    (void)SDL_SetWindowSize(window,width,height);
}

int main(int argc,char **argv) {
    SDL_Window *window=NULL;SDL_Renderer *renderer=NULL;struct nk_context *ctx;application app;ui_fonts fonts;
    bool running=true;float scale;int width=0,height=0;(void)argc;(void)argv;
    memset(&app,0,sizeof(app));memset(&fonts,0,sizeof(fonts));
    (void)snprintf(app.port,sizeof(app.port),"22");(void)snprintf(app.username,sizeof(app.username),"admin");
    (void)snprintf(app.remote_directory,sizeof(app.remote_directory),"/opt/omt-client");
    app.build_image=true;app.wifi_connect=true;
    app.mutex=SDL_CreateMutex();
    (void)SDL_SetAppMetadata("Raspberry Pi OMT Deployer",OMT_CLIENT_VERSION,"dev.mdmiller.rpi-omt-deployer");
    if(app.mutex==NULL||!SDL_Init(SDL_INIT_VIDEO)||
       !SDL_CreateWindowAndRenderer("Raspberry Pi OMT Deployer",(int)UI_DESIGN_WIDTH,(int)UI_DESIGN_HEIGHT,
                                    SDL_WINDOW_RESIZABLE|SDL_WINDOW_HIGH_PIXEL_DENSITY,&window,&renderer)){
        SDL_ShowSimpleMessageBox(SDL_MESSAGEBOX_ERROR,"Raspberry Pi OMT Deployer",SDL_GetError(),NULL);return 1;}
    /* SDL owns the base path it returns; the working directory is searched first
       so a packaged deployer started inside a checkout still finds that tree. */
    {const char *base=SDL_GetBasePath();char working[2048];char *root;
     if(working_directory(working,sizeof(working))==NULL)working[0]='\0';
     root=omt_discover_project_root(base,working);
     if(root!=NULL){(void)snprintf(app.project_root,sizeof(app.project_root),"%s",root);free(root);}}
    scale=window_scale(window);
    size_for_scale(window,scale);
    (void)SDL_SetWindowPosition(window,SDL_WINDOWPOS_CENTERED,SDL_WINDOWPOS_CENTERED);
    (void)SDL_SetRenderVSync(renderer,1);
    ctx=nk_sdl_init(window,renderer,nk_sdl_allocator());
    if(!build_fonts(&fonts,renderer,ctx,scale)){
        SDL_ShowSimpleMessageBox(SDL_MESSAGEBOX_ERROR,"Raspberry Pi OMT Deployer",
                                 "Unable to prepare the user interface font atlas.",window);
        nk_sdl_shutdown(ctx);SDL_DestroyRenderer(renderer);SDL_DestroyWindow(window);SDL_Quit();return 1;}
    apply_style(ctx,scale);
    refresh_checks(&app);

    nk_input_begin(ctx);
    while(running){
        SDL_Event event;uint64_t now;ui_metrics metrics;
        while(SDL_PollEvent(&event)){
            if(event.type==SDL_EVENT_QUIT||event.type==SDL_EVENT_WINDOW_CLOSE_REQUESTED)running=false;
            (void)SDL_ConvertEventToRenderCoordinates(renderer,&event);
            (void)nk_sdl_handle_event(ctx,&event);
        }
        nk_input_end(ctx);
        now=SDL_GetTicks();
        /* A window dragged to a differently scaled monitor rebakes its text at
           the new pixel height rather than being stretched. */
        {const float current=window_scale(window);
         if(SDL_fabsf(current-fonts.scale)>0.01F&&build_fonts(&fonts,renderer,ctx,current)){
             apply_style(ctx,current);apply_minimum_size(window,current);}}
        if(now-app.checked_at>=UI_CHECK_MILLISECONDS){refresh_checks(&app);app.checked_at=now;}
        (void)SDL_GetRenderOutputSize(renderer,&width,&height);
        metrics=measure(&fonts,fonts.scale,(float)width,(float)height);
        draw_ui(ctx,&app,&metrics,&fonts,(float)width,(float)height,now);
        (void)SDL_SetRenderDrawColor(renderer,ui_palette.canvas.r,ui_palette.canvas.g,ui_palette.canvas.b,255U);
        (void)SDL_RenderClear(renderer);
        nk_sdl_render(ctx,NK_ANTI_ALIASING_ON);
        nk_sdl_update_TextInput(ctx);
        SDL_RenderPresent(renderer);
        nk_input_begin(ctx);
    }
    SDL_SetAtomicInt(&app.cancel,1);if(app.thread!=NULL)SDL_WaitThread(app.thread,NULL);
    secure_buffer(app.password,sizeof(app.password));secure_buffer(app.key_passphrase,sizeof(app.key_passphrase));
    secure_buffer(app.sudo_password,sizeof(app.sudo_password));secure_buffer(app.wifi_password,sizeof(app.wifi_password));
    free(app.log);SDL_DestroyMutex(app.mutex);nk_input_end(ctx);
    release_fonts(&fonts);nk_sdl_shutdown(ctx);SDL_DestroyRenderer(renderer);
    SDL_DestroyWindow(window);SDL_Quit();return 0;
}
