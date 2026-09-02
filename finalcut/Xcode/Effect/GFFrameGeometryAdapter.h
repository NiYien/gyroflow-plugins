#import "GyroflowFinalCut.h"
#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>

NS_ASSUME_NONNULL_BEGIN

typedef struct GFFrameGeometryImageSnapshot {
    FxRect image_pixel_bounds;
    FxRect tile_pixel_bounds;
    __unsafe_unretained FxMatrix44 *pixel_transform;
    __unsafe_unretained FxMatrix44 *inverse_pixel_transform;
    FxImageOrigin image_origin;
    GFDimensionsU32 texture_dimensions;
} GFFrameGeometryImageSnapshot;

FOUNDATION_EXPORT BOOL GFFrameGeometryBuild(
    GFFrameGeometryImageSnapshot source,
    GFFrameGeometryImageSnapshot destination,
    GFProjectGeometry project,
    GFFrameGeometry *outGeometry,
    NSError **outError
);

NS_ASSUME_NONNULL_END
