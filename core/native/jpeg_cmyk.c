/* Minimal ABI boundary to the same task-local libjpeg used by OpenCV.
 * The decoder exposes CMYK samples; Rust owns color conversion and orientation.
 * No file access, Python calls, logging or model logic lives here. */
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <setjmp.h>
#include <jpeglib.h>

struct failure { struct jpeg_error_mgr base; jmp_buf jump; };
struct decoder { struct jpeg_decompress_struct jpeg; struct failure error; unsigned char *pixels; };
static void fail(j_common_ptr info) { longjmp(((struct failure *)info->err)->jump, 1); }
static void quiet(j_common_ptr info) { (void)info; }

/* Returns 1 for CMYK, 0 for another JPEG mode, -1 for malformed input.
 * Result is malloc-owned only on success and must be released by koma_jpeg_free. */
int koma_jpeg_cmyk(const unsigned char *input, size_t length,
                   unsigned char **result, unsigned int *width, unsigned int *height) {
    *result = NULL; *width = 0; *height = 0;
    struct decoder *state = calloc(1, sizeof(*state));
    if (!state) return -1;
    state->jpeg.err = jpeg_std_error(&state->error.base);
    state->error.base.error_exit = fail;
    state->error.base.output_message = quiet;
    if (setjmp(state->error.jump)) {
        if (state->jpeg.mem) jpeg_destroy_decompress(&state->jpeg);
        free(state->pixels); free(state); return -1;
    }
    jpeg_create_decompress(&state->jpeg);
    jpeg_mem_src(&state->jpeg, input, length);
    jpeg_read_header(&state->jpeg, TRUE);
    if (state->jpeg.num_components != 4) {
        jpeg_destroy_decompress(&state->jpeg); free(state); return 0;
    }
    state->jpeg.out_color_space = JCS_CMYK;
    jpeg_start_decompress(&state->jpeg);
    size_t w = state->jpeg.output_width, h = state->jpeg.output_height;
    if (!w || !h || w > 100000000 / h || w * h > SIZE_MAX / 4) {
        jpeg_destroy_decompress(&state->jpeg); free(state); return -1;
    }
    state->pixels = malloc(w * h * 4);
    if (!state->pixels) { jpeg_destroy_decompress(&state->jpeg); free(state); return -1; }
    while (state->jpeg.output_scanline < state->jpeg.output_height) {
        JSAMPROW row = state->pixels + state->jpeg.output_scanline * w * 4;
        jpeg_read_scanlines(&state->jpeg, &row, 1);
    }
    jpeg_finish_decompress(&state->jpeg);
    jpeg_destroy_decompress(&state->jpeg);
    *result = state->pixels; *width = (unsigned int)w; *height = (unsigned int)h;
    free(state); return 1;
}
void koma_jpeg_free(unsigned char *pixels) { free(pixels); }
