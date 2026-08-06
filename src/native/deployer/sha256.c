#include "deployer.h"

#include <stdio.h>
#include <string.h>

typedef struct {
    uint32_t state[8];
    unsigned char block[64];
    size_t used;
    uint64_t total;
} sha256_context;

static const uint32_t constants[64] = {
    0x428a2f98U,0x71374491U,0xb5c0fbcfU,0xe9b5dba5U,0x3956c25bU,0x59f111f1U,0x923f82a4U,0xab1c5ed5U,
    0xd807aa98U,0x12835b01U,0x243185beU,0x550c7dc3U,0x72be5d74U,0x80deb1feU,0x9bdc06a7U,0xc19bf174U,
    0xe49b69c1U,0xefbe4786U,0x0fc19dc6U,0x240ca1ccU,0x2de92c6fU,0x4a7484aaU,0x5cb0a9dcU,0x76f988daU,
    0x983e5152U,0xa831c66dU,0xb00327c8U,0xbf597fc7U,0xc6e00bf3U,0xd5a79147U,0x06ca6351U,0x14292967U,
    0x27b70a85U,0x2e1b2138U,0x4d2c6dfcU,0x53380d13U,0x650a7354U,0x766a0abbU,0x81c2c92eU,0x92722c85U,
    0xa2bfe8a1U,0xa81a664bU,0xc24b8b70U,0xc76c51a3U,0xd192e819U,0xd6990624U,0xf40e3585U,0x106aa070U,
    0x19a4c116U,0x1e376c08U,0x2748774cU,0x34b0bcb5U,0x391c0cb3U,0x4ed8aa4aU,0x5b9cca4fU,0x682e6ff3U,
    0x748f82eeU,0x78a5636fU,0x84c87814U,0x8cc70208U,0x90befffaU,0xa4506cebU,0xbef9a3f7U,0xc67178f2U
};

static uint32_t rotate(uint32_t value, unsigned bits) {
    return (value >> bits) | (value << (32U - bits));
}

static void transform(sha256_context *context, const unsigned char *data) {
    uint32_t words[64];
    uint32_t a,b,c,d,e,f,g,h;
    for (size_t i = 0U; i < 16U; ++i) {
        const size_t o = i * 4U;
        words[i] = ((uint32_t)data[o] << 24U) | ((uint32_t)data[o+1U] << 16U) |
                   ((uint32_t)data[o+2U] << 8U) | (uint32_t)data[o+3U];
    }
    for (size_t i = 16U; i < 64U; ++i) {
        const uint32_t s0 = rotate(words[i-15U],7U)^rotate(words[i-15U],18U)^(words[i-15U]>>3U);
        const uint32_t s1 = rotate(words[i-2U],17U)^rotate(words[i-2U],19U)^(words[i-2U]>>10U);
        words[i] = words[i-16U] + s0 + words[i-7U] + s1;
    }
    a=context->state[0]; b=context->state[1]; c=context->state[2]; d=context->state[3];
    e=context->state[4]; f=context->state[5]; g=context->state[6]; h=context->state[7];
    for (size_t i = 0U; i < 64U; ++i) {
        const uint32_t first=h+(rotate(e,6U)^rotate(e,11U)^rotate(e,25U))+((e&f)^((~e)&g))+constants[i]+words[i];
        const uint32_t second=(rotate(a,2U)^rotate(a,13U)^rotate(a,22U))+((a&b)^(a&c)^(b&c));
        h=g; g=f; f=e; e=d+first; d=c; c=b; b=a; a=first+second;
    }
    context->state[0]+=a; context->state[1]+=b; context->state[2]+=c; context->state[3]+=d;
    context->state[4]+=e; context->state[5]+=f; context->state[6]+=g; context->state[7]+=h;
}

static void update(sha256_context *context, const unsigned char *data, size_t size) {
    context->total += (uint64_t)size;
    while (size > 0U) {
        size_t count = 64U - context->used;
        if (count > size) count = size;
        memcpy(context->block + context->used, data, count);
        context->used += count; data += count; size -= count;
        if (context->used == 64U) { transform(context, context->block); context->used = 0U; }
    }
}

bool omt_sha256_file(const char *path, char output[65], char *error, size_t error_size) {
    sha256_context context = {{0x6a09e667U,0xbb67ae85U,0x3c6ef372U,0xa54ff53aU,
                               0x510e527fU,0x9b05688cU,0x1f83d9abU,0x5be0cd19U},{0},0U,0U};
    unsigned char buffer[65536];
    FILE *input = fopen(path, "rb");
    uint64_t bits;
    if (input == NULL) {
        if (error != NULL && error_size > 0U) (void)snprintf(error,error_size,"Unable to read deployment artifact: %s",path);
        return false;
    }
    for (;;) {
        const size_t count = fread(buffer, 1U, sizeof(buffer), input);
        if (count > 0U) update(&context, buffer, count);
        if (count < sizeof(buffer)) break;
    }
    if (ferror(input) != 0) {
        fclose(input);
        if (error != NULL && error_size > 0U) (void)snprintf(error,error_size,"Unable to finish reading deployment artifact.");
        return false;
    }
    fclose(input);
    bits = context.total * 8U;
    context.block[context.used++] = 0x80U;
    if (context.used > 56U) {
        memset(context.block + context.used, 0, 64U - context.used);
        transform(&context, context.block); context.used = 0U;
    }
    memset(context.block + context.used, 0, 56U - context.used);
    for (unsigned i = 0U; i < 8U; ++i) context.block[63U-i]=(unsigned char)(bits>>(i*8U));
    transform(&context, context.block);
    for (size_t i = 0U; i < 8U; ++i) (void)snprintf(output+i*8U,9U,"%08x",context.state[i]);
    output[64]='\0';
    omt_secure_clear((char *)&context, sizeof(context));
    omt_secure_clear((char *)buffer, sizeof(buffer));
    return true;
}
