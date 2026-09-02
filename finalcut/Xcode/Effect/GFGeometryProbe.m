// SPDX-License-Identifier: GPL-3.0-or-later

#import "GFGeometryProbe.h"

#import <os/log.h>
#import <stdatomic.h>

static const uint_fast32_t kGFGeometryProbeMaxRecordCount = 256;
static const NSUInteger kGFGeometryProbeMaxPayloadBytes = 8 * 1024;
static _Atomic(uint_fast32_t) gGFGeometryProbeRecordCount = 0;

@interface GFGeometryProbeSnapshot : NSObject

@property(nonatomic, readonly, copy) NSDictionary<NSString *, id> *payload;

- (instancetype)initWithSourceImage:(FxImageTile *)sourceImage
                   destinationImage:(FxImageTile *)destinationImage
                      sourceTexture:(id<MTLTexture>)sourceTexture
                 destinationTexture:(id<MTLTexture>)destinationTexture
                         renderTime:(CMTime)renderTime;

@end

static BOOL GFGeometryProbeIsEnabled(void) {
    static BOOL enabled = NO;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        NSString *value = [NSProcessInfo processInfo]
            .environment[@"GYROFLOW_FINALCUT_GEOMETRY_PROBE"]
            .lowercaseString;
        enabled = [@[@"1", @"true", @"yes", @"on"] containsObject:value];
    });
    return enabled;
}

static os_log_t GFGeometryProbeLog(void) {
    static os_log_t log;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        log = os_log_create("com.niyien.gyroflow.finalcut.effect", "geometry-probe");
    });
    return log;
}

static NSDictionary<NSString *, NSNumber *> *GFRectPayload(FxRect rect) {
    return @{
        @"left" : @(rect.left),
        @"bottom" : @(rect.bottom),
        @"right" : @(rect.right),
        @"top" : @(rect.top),
    };
}

static id GFMatrixPayload(FxMatrix44 *matrix) {
    if (matrix == nil) {
        return [NSNull null];
    }
    Matrix44Data *data = [matrix matrix];
    if (data == NULL) {
        return [NSNull null];
    }
    NSMutableArray<NSNumber *> *values = [NSMutableArray arrayWithCapacity:16];
    for (NSUInteger row = 0; row < 4; ++row) {
        for (NSUInteger column = 0; column < 4; ++column) {
            [values addObject:@((*data)[row][column])];
        }
    }
    return [values copy];
}

@implementation GFGeometryProbeSnapshot

- (instancetype)initWithSourceImage:(FxImageTile *)sourceImage
                   destinationImage:(FxImageTile *)destinationImage
                      sourceTexture:(id<MTLTexture>)sourceTexture
                 destinationTexture:(id<MTLTexture>)destinationTexture
                         renderTime:(CMTime)renderTime {
    self = [super init];
    if (self == nil) {
        return nil;
    }
    _payload = @{
        @"schema" : @"gyroflow-finalcut-geometry-probe-v1",
        @"renderTime" : @{
            @"value" : @(renderTime.value),
            @"timescale" : @(renderTime.timescale),
            @"flags" : @(renderTime.flags),
            @"epoch" : @(renderTime.epoch),
        },
        @"source" : @{
            @"pixelTransform" : GFMatrixPayload(sourceImage.pixelTransform),
            @"inversePixelTransform" : GFMatrixPayload(sourceImage.inversePixelTransform),
            @"imagePixelBounds" : GFRectPayload(sourceImage.imagePixelBounds),
            @"tilePixelBounds" : GFRectPayload(sourceImage.tilePixelBounds),
            @"imageOrigin" : @(sourceImage.imageOrigin),
            @"textureWidth" : @(sourceTexture.width),
            @"textureHeight" : @(sourceTexture.height),
        },
        @"destination" : @{
            @"pixelTransform" : GFMatrixPayload(destinationImage.pixelTransform),
            @"inversePixelTransform" : GFMatrixPayload(destinationImage.inversePixelTransform),
            @"imagePixelBounds" : GFRectPayload(destinationImage.imagePixelBounds),
            @"tilePixelBounds" : GFRectPayload(destinationImage.tilePixelBounds),
            @"imageOrigin" : @(destinationImage.imageOrigin),
            @"textureWidth" : @(destinationTexture.width),
            @"textureHeight" : @(destinationTexture.height),
        },
    };
    return self;
}

@end

void GFGeometryProbeRecordFrame(
    FxImageTile *sourceImage,
    FxImageTile *destinationImage,
    id<MTLTexture> sourceTexture,
    id<MTLTexture> destinationTexture,
    CMTime renderTime
) {
    if (!GFGeometryProbeIsEnabled()) {
        return;
    }
    uint_fast32_t sequence = atomic_fetch_add_explicit(
        &gGFGeometryProbeRecordCount,
        1,
        memory_order_relaxed
    );
    if (sequence >= kGFGeometryProbeMaxRecordCount) {
        atomic_store_explicit(
            &gGFGeometryProbeRecordCount,
            kGFGeometryProbeMaxRecordCount,
            memory_order_relaxed
        );
        return;
    }

    GFGeometryProbeSnapshot *snapshot = [[GFGeometryProbeSnapshot alloc]
        initWithSourceImage:sourceImage
           destinationImage:destinationImage
              sourceTexture:sourceTexture
         destinationTexture:destinationTexture
                 renderTime:renderTime];
    NSMutableDictionary<NSString *, id> *payload = [snapshot.payload mutableCopy];
    payload[@"sequence"] = @(sequence);
    NSError *error = nil;
    NSData *encoded = [NSJSONSerialization dataWithJSONObject:payload
                                                      options:0
                                                        error:&error];
    if (encoded == nil || encoded.length > kGFGeometryProbeMaxPayloadBytes) {
        os_log_info(GFGeometryProbeLog(),
                    "geometry_probe sequence=%{public}llu payload_rejected",
                    (unsigned long long)sequence);
        return;
    }
    NSString *json = [[NSString alloc] initWithData:encoded encoding:NSUTF8StringEncoding];
    if (json != nil) {
        os_log_info(GFGeometryProbeLog(),
                    "geometry_probe %{public}@",
                    json);
    }
}
