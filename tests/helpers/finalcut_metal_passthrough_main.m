// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <math.h>
#import "GFMetalPassthrough.h"

typedef _Float16 GFHalf;

static GFHostImageV2 GFImage(NSUInteger width, NSUInteger height, uint32_t origin,
                            NSUInteger left, NSUInteger bottom, NSUInteger right, NSUInteger top) {
    GFHostImageV2 image = {0};
    image.texture = (GFDimensionsU32){(uint32_t)width, (uint32_t)height};
    image.origin = origin;
    image.tile_rect[0] = 11;
    image.tile_rect[1] = 17;
    image.tile_rect[2] = 11 + width;
    image.tile_rect[3] = 17 + height;
    image.image_rect[0] = 11 + left;
    image.image_rect[1] = 17 + bottom;
    image.image_rect[2] = 11 + right;
    image.image_rect[3] = 17 + top;
    return image;
}

static id<MTLTexture> GFTexture(id<MTLDevice> device, NSUInteger w, NSUInteger h, BOOL output) {
    MTLTextureDescriptor *d = [MTLTextureDescriptor
        texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float width:w height:h mipmapped:NO];
    d.storageMode = output ? MTLStorageModePrivate : MTLStorageModeShared;
    d.usage = output ? MTLTextureUsageRenderTarget : MTLTextureUsageShaderRead;
    return [device newTextureWithDescriptor:d];
}

static float GFValue(NSUInteger x, NSUInteger y, NSUInteger channel, GFHostImageV2 image) {
    NSUInteger left = image.image_rect[0] - image.tile_rect[0];
    NSUInteger bottom = image.image_rect[1] - image.tile_rect[1];
    NSUInteger right = image.image_rect[2] - image.tile_rect[0];
    NSUInteger top = image.image_rect[3] - image.tile_rect[1];
    if (x < left || x >= right || y < bottom || y >= top) { return channel == 0 ? 1 : 0; }
    float visualY = image.origin == 0 ? top - y - 0.5f : y - bottom + 0.5f;
    if (channel == 0) { return 0.1f + 0.6f * (x - left + 0.5f) / (right - left); }
    if (channel == 1) { return 0.1f + 0.6f * visualY / (top - bottom); }
    if (channel == 2) { return 0.3f; }
    return 0.75f;
}

static BOOL GFCheck(id<MTLDevice> device, id<MTLCommandQueue> queue, GFMetalPassthrough *renderer,
                    GFHostImageV2 sourceImage, GFHostImageV2 destinationImage) {
    NSUInteger sw = sourceImage.texture.width, sh = sourceImage.texture.height;
    NSUInteger dw = destinationImage.texture.width, dh = destinationImage.texture.height;
    id<MTLTexture> input = GFTexture(device, sw, sh, NO);
    id<MTLTexture> output = GFTexture(device, dw, dh, YES);
    GFHalf *pixels = calloc(sw * sh * 4, sizeof(GFHalf));
    for (NSUInteger y = 0; y < sh; y++) for (NSUInteger x = 0; x < sw; x++)
        for (NSUInteger c = 0; c < 4; c++) pixels[(y * sw + x) * 4 + c] = GFValue(x,y,c,sourceImage);
    [input replaceRegion:MTLRegionMake2D(0,0,sw,sh) mipmapLevel:0
               withBytes:pixels bytesPerRow:sw * 4 * sizeof(GFHalf)];
    free(pixels);
    NSError *error = nil;
    if (![renderer copySource:input sourceImage:sourceImage destination:output
              destinationImage:destinationImage commandQueue:queue gpuSeconds:NULL error:&error]) {
        fprintf(stderr,"copy failed: %s\n",error.localizedDescription.UTF8String);
        return NO;
    }
    id<MTLTexture> readback = GFTexture(device, dw, dh, NO);
    id<MTLCommandBuffer> command = [queue commandBuffer];
    id<MTLBlitCommandEncoder> blit = [command blitCommandEncoder];
    [blit copyFromTexture:output sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0)
              sourceSize:MTLSizeMake(dw,dh,1) toTexture:readback destinationSlice:0
         destinationLevel:0 destinationOrigin:MTLOriginMake(0,0,0)];
    [blit endEncoding]; [command commit]; [command waitUntilCompleted];
    GFHalf *actual = calloc(dw * dh * 4, sizeof(GFHalf));
    [readback getBytes:actual bytesPerRow:dw * 4 * sizeof(GFHalf)
            fromRegion:MTLRegionMake2D(0,0,dw,dh) mipmapLevel:0];
    double sl = sourceImage.image_rect[0] - sourceImage.tile_rect[0];
    double sb = sourceImage.image_rect[1] - sourceImage.tile_rect[1];
    double sr = sourceImage.image_rect[2] - sourceImage.tile_rect[0];
    double st = sourceImage.image_rect[3] - sourceImage.tile_rect[1];
    double dl = destinationImage.image_rect[0] - destinationImage.tile_rect[0];
    double db = destinationImage.image_rect[1] - destinationImage.tile_rect[1];
    double dr = destinationImage.image_rect[2] - destinationImage.tile_rect[0];
    double dt = destinationImage.image_rect[3] - destinationImage.tile_rect[1];
    BOOL ok = YES;
    for (NSUInteger y = 0; y < dh && ok; y++) for (NSUInteger x = 0; x < dw && ok; x++) {
        BOOL inside = x + 0.5 >= dl && x + 0.5 < dr && y + 0.5 >= db && y + 0.5 < dt;
        double sx = sl + (x + 0.5 - dl) / (dr - dl) * (sr - sl);
        double sy = sb + (y + 0.5 - db) / (dt - db) * (st - sb);
        if (sourceImage.origin != destinationImage.origin) { sy = st - (sy - sb); }
        sx = fmax(sl + 0.5, fmin(sr - 0.5, sx)) - 0.5;
        sy = fmax(sb + 0.5, fmin(st - 0.5, sy)) - 0.5;
        NSUInteger x0 = floor(sx), y0 = floor(sy);
        NSUInteger x1 = MIN(x0 + 1, (NSUInteger)sr - 1), y1 = MIN(y0 + 1, (NSUInteger)st - 1);
        double fx = sx - x0, fy = sy - y0;
        for (NSUInteger c = 0; c < 4; c++) {
            double expected = inside ?
                (1-fy)*((1-fx)*GFValue(x0,y0,c,sourceImage)+fx*GFValue(x1,y0,c,sourceImage)) +
                fy*((1-fx)*GFValue(x0,y1,c,sourceImage)+fx*GFValue(x1,y1,c,sourceImage)) : 0;
            double value = actual[(y * dw + x) * 4 + c];
            if (!isfinite(value) || fabs(value - expected) > 0.002) {
                fprintf(stderr,"pixel mismatch %lux%lu -> %lux%lu origin=%u/%u at %lu,%lu,%lu: %.5f expected %.5f\n",
                    sw,sh,dw,dh,sourceImage.origin,destinationImage.origin,x,y,c,value,expected);
                ok = NO;
                break;
            }
        }
    }
    free(actual);
    return ok;
}

int main(void) {
    @autoreleasepool {
        id<MTLDevice> device = MTLCreateSystemDefaultDevice();
        if (device == nil) { return 77; }
        id<MTLCommandQueue> queue = [device newCommandQueue];
        GFMetalPassthrough *renderer = [[GFMetalPassthrough alloc] initWithDevice:device];
        NSUInteger cases = 0;
        for (uint32_t s = 0; s <= 2; s += 2) for (uint32_t d = 0; d <= 2; d += 2) {
            GFHostImageV2 source = GFImage(16,12,s,0,0,16,12);
            for (NSUInteger scale = 0; scale < 3; scale++) {
                NSUInteger w = scale == 0 ? 16 : (scale == 1 ? 8 : 32);
                NSUInteger h = scale == 0 ? 12 : (scale == 1 ? 6 : 24);
                if (!GFCheck(device,queue,renderer,source,GFImage(w,h,d,0,0,w,h))) { return 2; }
                cases++;
            }
            if (!GFCheck(device,queue,renderer,GFImage(16,12,s,2,3,14,10),GFImage(10,9,d,1,2,9,7))) { return 3; }
            cases++;
        }
        GFHostImageV2 invalid = GFImage(16,12,2,0,0,16,12);
        invalid.texture.width = 15;
        NSError *error = nil;
        if ([renderer copySource:GFTexture(device,16,12,NO) sourceImage:invalid
                  destination:GFTexture(device,8,6,YES) destinationImage:GFImage(8,6,2,0,0,8,6)
                  commandQueue:queue gpuSeconds:NULL error:&error] || error == nil) { return 4; }
        printf("%lu real Metal passthrough image cases passed; invalid metadata rejected\n",cases);
    }
    return 0;
}
