#include <SDL3/SDL.h>

#define NK_INCLUDE_STANDARD_VARARGS
#define NK_INCLUDE_STANDARD_IO
#define NK_INCLUDE_FONT_BAKING
#define NK_INCLUDE_DEFAULT_FONT
#define NK_INCLUDE_COMMAND_USERDATA
#define NK_INCLUDE_VERTEX_BUFFER_OUTPUT
#define NK_INCLUDE_DEFAULT_ALLOCATOR
static char *nk_sdl_dtoa(char *text, double value);
#define NK_DTOA(text, value) nk_sdl_dtoa((text), (value))
#define NK_IMPLEMENTATION
#include <nuklear.h>

static char *nk_sdl_dtoa(char *text, double value) {
    if (text != NULL) {
        (void)SDL_snprintf(text, 99999U, "%.17g", value);
    }
    return text;
}

#define NK_SDL3_RENDERER_IMPLEMENTATION
#include <nuklear_sdl3_renderer.h>
