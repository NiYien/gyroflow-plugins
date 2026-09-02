#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>
#import <math.h>

#import "GFFrameGeometryAdapter.h"

@interface GFTestMatrix : NSObject
@property(nonatomic) double a;
@property(nonatomic) double b;
@property(nonatomic) double c;
@property(nonatomic) double d;
@property(nonatomic) double tx;
@property(nonatomic) double ty;
@property(nonatomic) double perspective;
+ (instancetype)matrixWithA:(double)a
                          b:(double)b
                          c:(double)c
                          d:(double)d
                         tx:(double)tx
                         ty:(double)ty;
@end

@implementation GFTestMatrix
+ (instancetype)matrixWithA:(double)a
                          b:(double)b
                          c:(double)c
                          d:(double)d
                         tx:(double)tx
                         ty:(double)ty {
    GFTestMatrix *matrix = [[self alloc] init];
    matrix.a = a;
    matrix.b = b;
    matrix.c = c;
    matrix.d = d;
    matrix.tx = tx;
    matrix.ty = ty;
    return matrix;
}
- (FxPoint2D)transform2DPoint:(FxPoint2D)point {
    double denominator = 1.0 + self.perspective * point.x;
    return (FxPoint2D){
        .x = (self.a * point.x + self.b * point.y + self.tx) / denominator,
        .y = (self.c * point.x + self.d * point.y + self.ty) / denominator,
    };
}
@end

@interface GFTestTile : NSObject
@property(nonatomic) FxRect imagePixelBounds;
@property(nonatomic) FxRect tilePixelBounds;
@property(nonatomic, strong) GFTestMatrix *pixelTransform;
@property(nonatomic, strong) GFTestMatrix *inversePixelTransform;
@property(nonatomic) FxImageOrigin imageOrigin;
@end

@implementation GFTestTile
@end

static BOOL GFNear(double actual, double expected) {
    return fabs(actual - expected) <= 1.0e-9;
}

static GFTestTile *GFTile(
    FxRect imageBounds,
    FxRect tileBounds,
    GFTestMatrix *forward,
    GFTestMatrix *inverse,
    FxImageOrigin origin
) {
    GFTestTile *tile = [[GFTestTile alloc] init];
    tile.imagePixelBounds = imageBounds;
    tile.tilePixelBounds = tileBounds;
    tile.pixelTransform = forward;
    tile.inversePixelTransform = inverse;
    tile.imageOrigin = origin;
    return tile;
}

int main(void) {
    @autoreleasepool {
        GFProjectGeometry project = {
            .input_dimensions = {.width = 3840, .height = 2160},
            .output_dimensions = {.width = 3840, .height = 2160},
            .video_rotation = GF_ROTATION_NONE,
        };
        FxRect full = {.left = 0, .bottom = 0, .right = 960, .top = 540};
        GFTestTile *source = GFTile(
            full,
            full,
            [GFTestMatrix matrixWithA:0.25 b:0.0 c:0.0 d:0.25 tx:0.0 ty:0.0],
            [GFTestMatrix matrixWithA:4.0 b:0.0 c:0.0 d:4.0 tx:0.0 ty:0.0],
            kFxImageOrigin_TOP_LEFT
        );
        GFTestTile *destination = GFTile(
            full,
            full,
            [GFTestMatrix matrixWithA:0.5 b:0.0 c:0.0 d:0.5 tx:0.0 ty:0.0],
            [GFTestMatrix matrixWithA:2.0 b:0.0 c:0.0 d:2.0 tx:0.0 ty:0.0],
            kFxImageOrigin_TOP_LEFT
        );
        GFDimensionsU32 texture = {.width = 960, .height = 540};
        GFFrameGeometry first = {0};
        NSError *error = nil;
        GFFrameGeometryImageSnapshot sourceSnapshot = {
            .image_pixel_bounds = source.imagePixelBounds,
            .tile_pixel_bounds = source.tilePixelBounds,
            .pixel_transform = (FxMatrix44 *)source.pixelTransform,
            .inverse_pixel_transform = (FxMatrix44 *)source.inversePixelTransform,
            .image_origin = source.imageOrigin,
            .texture_dimensions = texture,
        };
        GFFrameGeometryImageSnapshot destinationSnapshot = {
            .image_pixel_bounds = destination.imagePixelBounds,
            .tile_pixel_bounds = destination.tilePixelBounds,
            .pixel_transform = (FxMatrix44 *)destination.pixelTransform,
            .inverse_pixel_transform = (FxMatrix44 *)destination.inversePixelTransform,
            .image_origin = destination.imageOrigin,
            .texture_dimensions = texture,
        };
        BOOL firstOK = GFFrameGeometryBuild(
            sourceSnapshot, destinationSnapshot, project, &first, &error
        );
        if (!firstOK) {
            fprintf(stderr, "%s\n", error.localizedDescription.UTF8String);
            return 2;
        }
        BOOL normalized =
            first.version == GF_FRAME_GEOMETRY_VERSION &&
            first.validity == GF_GEOMETRY_VALIDITY_VALID &&
            first.support == GF_GEOMETRY_SUPPORT_AFFINE_2D &&
            first.source_dimensions.width == 3840 &&
            first.oriented_dimensions.width == 3840 &&
            first.tile_dimensions.width == 960 &&
            first.output_dimensions.width == 1920 &&
            first.output_dimensions.height == 1080 &&
            first.source_rect.right == 3840 &&
            first.source_rect.top == 2160 &&
            first.destination_rect.right == 1920 &&
            first.destination_rect.top == 1080 &&
            GFNear(first.forward_transform.values[0], 0.5) &&
            GFNear(first.forward_transform.values[4], 0.5) &&
            GFNear(first.inverse_transform.values[0], 2.0) &&
            GFNear(first.inverse_transform.values[4], 2.0);

        destination.pixelTransform =
            [GFTestMatrix matrixWithA:0.5 b:0.0 c:0.0 d:0.5 tx:-20.0 ty:-10.0];
        destination.inversePixelTransform =
            [GFTestMatrix matrixWithA:2.0 b:0.0 c:0.0 d:2.0 tx:40.0 ty:20.0];
        destinationSnapshot.pixel_transform =
            (FxMatrix44 *)destination.pixelTransform;
        destinationSnapshot.inverse_pixel_transform =
            (FxMatrix44 *)destination.inversePixelTransform;
        GFFrameGeometry second = {0};
        BOOL secondOK = GFFrameGeometryBuild(
            sourceSnapshot, destinationSnapshot, project, &second, &error
        );
        BOOL changedImmediately = secondOK &&
            !GFNear(first.forward_transform.values[2], second.forward_transform.values[2]);

        GFTestMatrix *perspective =
            [GFTestMatrix matrixWithA:1.0 b:0.0 c:0.0 d:1.0 tx:0.0 ty:0.0];
        perspective.perspective = 0.001;
        destination.pixelTransform = perspective;
        destination.inversePixelTransform =
            [GFTestMatrix matrixWithA:1.0 b:0.0 c:0.0 d:1.0 tx:0.0 ty:0.0];
        destinationSnapshot.pixel_transform =
            (FxMatrix44 *)destination.pixelTransform;
        destinationSnapshot.inverse_pixel_transform =
            (FxMatrix44 *)destination.inversePixelTransform;
        GFFrameGeometry rejected = {0};
        BOOL invalidRejected = !GFFrameGeometryBuild(
            sourceSnapshot, destinationSnapshot, project, &rejected, &error
        ) && rejected.validity == GF_GEOMETRY_VALIDITY_INVALID &&
            rejected.support == GF_GEOMETRY_SUPPORT_UNSUPPORTED && error != nil;

        GFTestMatrix *rotatedSourceForward =
            [GFTestMatrix matrixWithA:0.25 b:0.0 c:0.0 d:0.25 tx:0.0 ty:0.0];
        GFTestMatrix *rotatedSourceInverse =
            [GFTestMatrix matrixWithA:4.0 b:0.0 c:0.0 d:4.0 tx:0.0 ty:0.0];
        GFTestMatrix *rotatedDestinationForward =
            [GFTestMatrix matrixWithA:(4.0 / 9.0) b:0.0 c:0.0 d:(9.0 / 64.0) tx:0.0 ty:0.0];
        GFTestMatrix *rotatedDestinationInverse =
            [GFTestMatrix matrixWithA:2.25 b:0.0 c:0.0 d:(64.0 / 9.0) tx:0.0 ty:0.0];
        sourceSnapshot.pixel_transform = (FxMatrix44 *)rotatedSourceForward;
        sourceSnapshot.inverse_pixel_transform = (FxMatrix44 *)rotatedSourceInverse;
        destinationSnapshot.pixel_transform = (FxMatrix44 *)rotatedDestinationForward;
        destinationSnapshot.inverse_pixel_transform = (FxMatrix44 *)rotatedDestinationInverse;
        BOOL quarterTurnsSupported = YES;
        for (GFRotationDegrees rotation = GF_ROTATION_CLOCKWISE_90;
             rotation <= GF_ROTATION_CLOCKWISE_270;
             rotation += 180) {
            GFProjectGeometry rotatedProject = {
                .input_dimensions = {.width = 3840, .height = 2160},
                .output_dimensions = {.width = 2160, .height = 3840},
                .video_rotation = rotation,
            };
            GFFrameGeometry rotated = {0};
            BOOL rotatedOK = GFFrameGeometryBuild(
                sourceSnapshot,
                destinationSnapshot,
                rotatedProject,
                &rotated,
                &error
            );
            quarterTurnsSupported = quarterTurnsSupported && rotatedOK &&
                rotated.input_rotation == rotation &&
                rotated.video_rotation == rotation &&
                rotated.source_rect.left == 0 && rotated.source_rect.bottom == 0 &&
                rotated.source_rect.right == 2160 && rotated.source_rect.top == 3840;
        }

        FxRect portraitBounds = {
            .left = 0,
            .bottom = 0,
            .right = 540,
            .top = 960,
        };
        sourceSnapshot.image_pixel_bounds = portraitBounds;
        sourceSnapshot.tile_pixel_bounds = portraitBounds;
        sourceSnapshot.texture_dimensions = (GFDimensionsU32){.width = 540, .height = 960};
        GFTestMatrix *portraitForward =
            [GFTestMatrix matrixWithA:0.5 b:0.0 c:0.0 d:0.5 tx:0.0 ty:0.0];
        GFTestMatrix *portraitInverse =
            [GFTestMatrix matrixWithA:2.0 b:0.0 c:0.0 d:2.0 tx:0.0 ty:0.0];
        sourceSnapshot.pixel_transform = (FxMatrix44 *)portraitForward;
        sourceSnapshot.inverse_pixel_transform = (FxMatrix44 *)portraitInverse;
        destinationSnapshot = sourceSnapshot;
        GFProjectGeometry portraitProject = {
            .input_dimensions = {.width = 1080, .height = 1920},
            .output_dimensions = {.width = 1080, .height = 1920},
            .video_rotation = GF_ROTATION_NONE,
        };
        GFFrameGeometry portrait = {0};
        BOOL portraitSupported = GFFrameGeometryBuild(
            sourceSnapshot,
            destinationSnapshot,
            portraitProject,
            &portrait,
            &error
        ) && portrait.input_rotation == GF_ROTATION_NONE &&
            portrait.source_dimensions.width == 1080 &&
            portrait.source_dimensions.height == 1920;

        sourceSnapshot.image_pixel_bounds = full;
        sourceSnapshot.tile_pixel_bounds = full;
        sourceSnapshot.texture_dimensions = texture;
        sourceSnapshot.pixel_transform = (FxMatrix44 *)portraitForward;
        sourceSnapshot.inverse_pixel_transform = (FxMatrix44 *)portraitInverse;
        GFTestMatrix *paspDestinationForward =
            [GFTestMatrix matrixWithA:(960.0 / 2554.0) b:0.0 c:0.0 d:0.5 tx:0.0 ty:0.0];
        GFTestMatrix *paspDestinationInverse =
            [GFTestMatrix matrixWithA:(2554.0 / 960.0) b:0.0 c:0.0 d:2.0 tx:0.0 ty:0.0];
        destinationSnapshot.image_pixel_bounds = full;
        destinationSnapshot.tile_pixel_bounds = full;
        destinationSnapshot.texture_dimensions = texture;
        destinationSnapshot.pixel_transform = (FxMatrix44 *)paspDestinationForward;
        destinationSnapshot.inverse_pixel_transform = (FxMatrix44 *)paspDestinationInverse;
        GFProjectGeometry paspProject = {
            .input_dimensions = {.width = 1920, .height = 1080},
            .output_dimensions = {.width = 2554, .height = 1080},
            .video_rotation = GF_ROTATION_NONE,
        };
        GFFrameGeometry pasp = {0};
        BOOL paspSupported = GFFrameGeometryBuild(
            sourceSnapshot,
            destinationSnapshot,
            paspProject,
            &pasp,
            &error
        ) && pasp.input_rotation == GF_ROTATION_NONE &&
            pasp.source_rect.right == 2554 && pasp.source_rect.top == 1080 &&
            pasp.oriented_dimensions.width == 2554;

        FxRect playbackImageBounds = {
            .left = 0,
            .bottom = 0,
            .right = 960,
            .top = 540,
        };
        FxRect playbackTileBounds = {
            .left = -2,
            .bottom = -4,
            .right = 964,
            .top = 542,
        };
        GFTestMatrix *playbackPixelTransform =
            [GFTestMatrix matrixWithA:0.5 b:0.0 c:0.0 d:0.5 tx:480.0 ty:270.0];
        GFTestMatrix *playbackInversePixelTransform =
            [GFTestMatrix matrixWithA:2.0 b:0.0 c:0.0 d:2.0 tx:-960.0 ty:-540.0];
        GFFrameGeometryImageSnapshot playbackSnapshot = {
            .image_pixel_bounds = playbackImageBounds,
            .tile_pixel_bounds = playbackTileBounds,
            .pixel_transform = (FxMatrix44 *)playbackPixelTransform,
            .inverse_pixel_transform = (FxMatrix44 *)playbackInversePixelTransform,
            .image_origin = kFxImageOrigin_BOTTOM_LEFT,
            .texture_dimensions = {.width = 966, .height = 546},
        };
        GFProjectGeometry playbackProject = {
            .input_dimensions = {.width = 1920, .height = 1080},
            .output_dimensions = {.width = 1920, .height = 1080},
            .video_rotation = GF_ROTATION_NONE,
        };
        GFFrameGeometry playback = {0};
        BOOL playbackProxySupported = GFFrameGeometryBuild(
            playbackSnapshot,
            playbackSnapshot,
            playbackProject,
            &playback,
            &error
        ) && playback.source_dimensions.width == 1920 &&
            playback.source_dimensions.height == 1080 &&
            playback.tile_dimensions.width == 966 &&
            playback.tile_dimensions.height == 546;

        FxRect liveFullImageBounds = {
            .left = 0,
            .bottom = 0,
            .right = 1920,
            .top = 1080,
        };
        FxRect liveFullTileBounds = {
            .left = -2,
            .bottom = -4,
            .right = 1924,
            .top = 1082,
        };
        GFTestMatrix *liveFullPixelTransform =
            [GFTestMatrix matrixWithA:1.0 b:0.0 c:0.0 d:1.0 tx:960.0 ty:540.0];
        GFTestMatrix *liveFullInversePixelTransform =
            [GFTestMatrix matrixWithA:1.0 b:0.0 c:0.0 d:1.0 tx:-960.0 ty:-540.0];
        GFFrameGeometryImageSnapshot liveFullSnapshot = {
            .image_pixel_bounds = liveFullImageBounds,
            .tile_pixel_bounds = liveFullTileBounds,
            .pixel_transform = (FxMatrix44 *)liveFullPixelTransform,
            .inverse_pixel_transform = (FxMatrix44 *)liveFullInversePixelTransform,
            .image_origin = kFxImageOrigin_BOTTOM_LEFT,
            .texture_dimensions = {.width = 1926, .height = 1086},
        };
        GFProjectGeometry sidewaysProject = {
            .input_dimensions = {.width = 1920, .height = 1080},
            .output_dimensions = {.width = 1080, .height = 1920},
            .video_rotation = GF_ROTATION_CLOCKWISE_90,
        };
        GFFrameGeometry unsupportedSideways = {0};
        BOOL unsupportedLandscapeCanvasRejected = !GFFrameGeometryBuild(
            liveFullSnapshot,
            liveFullSnapshot,
            sidewaysProject,
            &unsupportedSideways,
            &error
        ) && unsupportedSideways.validity == GF_GEOMETRY_VALIDITY_INVALID;

        if (!normalized || !changedImmediately || !invalidRejected ||
            !quarterTurnsSupported || !portraitSupported || !paspSupported ||
            !playbackProxySupported || !unsupportedLandscapeCanvasRejected) {
            fprintf(stderr,
                    "normalized=%d changedImmediately=%d invalidRejected=%d quarterTurnsSupported=%d portraitSupported=%d paspSupported=%d playbackProxySupported=%d unsupportedLandscapeCanvasRejected=%d\n",
                    normalized,
                    changedImmediately,
                    invalidRejected,
                    quarterTurnsSupported,
                    portraitSupported,
                    paspSupported,
                    playbackProxySupported,
                    unsupportedLandscapeCanvasRejected);
            return 3;
        }
        return 0;
    }
}
