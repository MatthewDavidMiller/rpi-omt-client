// Copyright (c) 2026 Matthew David Miller
// SPDX-License-Identifier: MIT
// Minimal C declaration surface for the vendored VMX decoder.
#ifndef OMT_RECEIVER_VMX_API_H
#define OMT_RECEIVER_VMX_API_H

typedef struct VMX_INSTANCE VMX_INSTANCE;
typedef struct { int width; int height; } VMX_SIZE;
typedef enum {
    VMX_ERR_OK,
    VMX_ERR_UNKNOWN,
    VMX_ERR_INVALID_CODEC_FORMAT,
    VMX_ERR_INVALID_SLICE_COUNT,
    VMX_ERR_BUFFER_OVERFLOW,
    VMX_ERR_INVALID_INSTANCE,
    VMX_ERR_INVALID_PARAMETERS
} VMX_ERR;
typedef enum { VMX_PROFILE_OMT_SQ = 166 } VMX_PROFILE;
typedef enum {
    VMX_COLORSPACE_UNDEFINED = 0,
    VMX_COLORSPACE_BT601 = 601,
    VMX_COLORSPACE_BT709 = 709
} VMX_COLORSPACE;

VMX_INSTANCE *VMX_Create(VMX_SIZE dimensions, VMX_PROFILE profile, VMX_COLORSPACE color_space);
void VMX_Destroy(VMX_INSTANCE *instance);
VMX_ERR VMX_LoadFrom(VMX_INSTANCE *instance, unsigned char *data, int data_length);
VMX_ERR VMX_DecodeBGRX(VMX_INSTANCE *instance, unsigned char *destination, int stride);

#endif
