#import "GFFrameGeometryAdapter.h"

#import <float.h>
#import <math.h>

static NSString *const GFFrameGeometryAdapterErrorDomain =
    @"com.niyien.gyroflow.finalcut.frame-geometry";

typedef struct GFAdapterAffine {
    double values[9];
} GFAdapterAffine;

static BOOL GFFail(
    GFFrameGeometry *outGeometry,
    NSError **outError,
    NSString *message
) {
    if (outGeometry != NULL) {
        *outGeometry = (GFFrameGeometry){
            .version = GF_FRAME_GEOMETRY_VERSION,
            .struct_size = (uint32_t)sizeof(GFFrameGeometry),
            .validity = GF_GEOMETRY_VALIDITY_INVALID,
            .support = GF_GEOMETRY_SUPPORT_UNSUPPORTED,
            .source_origin = GF_IMAGE_ORIGIN_UNKNOWN,
            .destination_origin = GF_IMAGE_ORIGIN_UNKNOWN,
            .input_rotation = GF_ROTATION_UNKNOWN,
            .video_rotation = GF_ROTATION_UNKNOWN,
        };
    }
    if (outError != NULL) {
        *outError = [NSError errorWithDomain:GFFrameGeometryAdapterErrorDomain
                                        code:1
                                    userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return NO;
}

static BOOL GFFinitePoint(FxPoint2D point) {
    return isfinite(point.x) && isfinite(point.y);
}

static FxPoint2D GFApply(const GFAdapterAffine *transform, FxPoint2D point) {
    return (FxPoint2D){
        .x = transform->values[0] * point.x +
             transform->values[1] * point.y + transform->values[2],
        .y = transform->values[3] * point.x +
             transform->values[4] * point.y + transform->values[5],
    };
}

static BOOL GFNear(double actual, double expected) {
    double scale = fmax(fmax(fabs(actual), fabs(expected)), 1.0);
    return fabs(actual - expected) <= 1.0e-8 * scale;
}

static BOOL GFExtractAffine(
    FxMatrix44 *matrix,
    FxRect bounds,
    GFAdapterAffine *outTransform
) {
    if (matrix == nil || outTransform == NULL) {
        return NO;
    }
    FxPoint2D origin = [matrix transform2DPoint:(FxPoint2D){.x = 0.0, .y = 0.0}];
    FxPoint2D xBasis = [matrix transform2DPoint:(FxPoint2D){.x = 1.0, .y = 0.0}];
    FxPoint2D yBasis = [matrix transform2DPoint:(FxPoint2D){.x = 0.0, .y = 1.0}];
    if (!GFFinitePoint(origin) || !GFFinitePoint(xBasis) || !GFFinitePoint(yBasis)) {
        return NO;
    }
    *outTransform = (GFAdapterAffine){.values = {
        xBasis.x - origin.x,
        yBasis.x - origin.x,
        origin.x,
        xBasis.y - origin.y,
        yBasis.y - origin.y,
        origin.y,
        0.0,
        0.0,
        1.0,
    }};
    FxPoint2D probes[] = {
        {.x = 2.0, .y = 0.0},
        {.x = 0.0, .y = 2.0},
        {.x = 1.0, .y = 1.0},
        {.x = bounds.left, .y = bounds.bottom},
        {.x = bounds.left, .y = bounds.top},
        {.x = bounds.right, .y = bounds.bottom},
        {.x = bounds.right, .y = bounds.top},
        {
            .x = ((double)bounds.left + (double)bounds.right) * 0.5,
            .y = ((double)bounds.bottom + (double)bounds.top) * 0.5,
        },
    };
    for (NSUInteger index = 0; index < sizeof(probes) / sizeof(probes[0]); ++index) {
        FxPoint2D actual = [matrix transform2DPoint:probes[index]];
        FxPoint2D expected = GFApply(outTransform, probes[index]);
        if (!GFFinitePoint(actual) || !GFNear(actual.x, expected.x) ||
            !GFNear(actual.y, expected.y)) {
            return NO;
        }
    }
    return YES;
}

static GFAdapterAffine GFMultiply(
    const GFAdapterAffine *left,
    const GFAdapterAffine *right
) {
    GFAdapterAffine product = {0};
    for (NSUInteger row = 0; row < 3; ++row) {
        for (NSUInteger column = 0; column < 3; ++column) {
            for (NSUInteger index = 0; index < 3; ++index) {
                product.values[row * 3 + column] +=
                    left->values[row * 3 + index] *
                    right->values[index * 3 + column];
            }
        }
    }
    return product;
}

static BOOL GFIdentity(const GFAdapterAffine *transform) {
    static const double identity[9] = {
        1.0, 0.0, 0.0,
        0.0, 1.0, 0.0,
        0.0, 0.0, 1.0,
    };
    for (NSUInteger index = 0; index < 9; ++index) {
        if (!GFNear(transform->values[index], identity[index])) {
            return NO;
        }
    }
    return YES;
}

static BOOL GFScaleTranslate(const GFAdapterAffine *transform) {
    return transform->values[0] > 0.0 && transform->values[4] > 0.0 &&
           GFNear(transform->values[1], 0.0) &&
           GFNear(transform->values[3], 0.0) &&
           GFNear(transform->values[6], 0.0) &&
           GFNear(transform->values[7], 0.0) &&
           GFNear(transform->values[8], 1.0);
}

static BOOL GFPositiveRect(FxRect rect) {
    return rect.right > rect.left && rect.top > rect.bottom;
}

static BOOL GFRoundCoordinate(double value, int32_t *outValue) {
    if (!isfinite(value) || value < INT32_MIN || value > INT32_MAX) {
        return NO;
    }
    double rounded = round(value);
    if (!GFNear(value, rounded)) {
        return NO;
    }
    *outValue = (int32_t)rounded;
    return YES;
}

static BOOL GFTransformRect(
    const GFAdapterAffine *transform,
    FxRect rect,
    GFRectI32 *outRect
) {
    FxPoint2D corners[] = {
        {.x = rect.left, .y = rect.bottom},
        {.x = rect.left, .y = rect.top},
        {.x = rect.right, .y = rect.bottom},
        {.x = rect.right, .y = rect.top},
    };
    double left = DBL_MAX;
    double bottom = DBL_MAX;
    double right = -DBL_MAX;
    double top = -DBL_MAX;
    for (NSUInteger index = 0; index < sizeof(corners) / sizeof(corners[0]); ++index) {
        FxPoint2D point = GFApply(transform, corners[index]);
        if (!GFFinitePoint(point)) {
            return NO;
        }
        left = fmin(left, point.x);
        bottom = fmin(bottom, point.y);
        right = fmax(right, point.x);
        top = fmax(top, point.y);
    }
    return GFRoundCoordinate(left, &outRect->left) &&
           GFRoundCoordinate(bottom, &outRect->bottom) &&
           GFRoundCoordinate(right, &outRect->right) &&
           GFRoundCoordinate(top, &outRect->top) &&
           outRect->right > outRect->left && outRect->top > outRect->bottom;
}

static BOOL GFRectDimensions(GFRectI32 rect, GFDimensionsU32 *outDimensions) {
    int64_t width = (int64_t)rect.right - (int64_t)rect.left;
    int64_t height = (int64_t)rect.top - (int64_t)rect.bottom;
    if (width <= 0 || height <= 0 || width > UINT32_MAX || height > UINT32_MAX) {
        return NO;
    }
    *outDimensions = (GFDimensionsU32){
        .width = (uint32_t)width,
        .height = (uint32_t)height,
    };
    return YES;
}

static BOOL GFDimensionsEqual(GFDimensionsU32 left, GFDimensionsU32 right) {
    return left.width == right.width && left.height == right.height;
}

static BOOL GFOrientRect(
    GFRectI32 rect,
    GFRectI32 imageRect,
    GFRotationDegrees rotation,
    GFRectI32 *outRect
) {
    double width = (double)imageRect.right - (double)imageRect.left;
    double height = (double)imageRect.top - (double)imageRect.bottom;
    FxPoint2D corners[] = {
        {.x = rect.left - imageRect.left, .y = rect.bottom - imageRect.bottom},
        {.x = rect.left - imageRect.left, .y = rect.top - imageRect.bottom},
        {.x = rect.right - imageRect.left, .y = rect.bottom - imageRect.bottom},
        {.x = rect.right - imageRect.left, .y = rect.top - imageRect.bottom},
    };
    double left = DBL_MAX;
    double bottom = DBL_MAX;
    double right = -DBL_MAX;
    double top = -DBL_MAX;
    for (NSUInteger index = 0; index < sizeof(corners) / sizeof(corners[0]); ++index) {
        FxPoint2D point = corners[index];
        switch (rotation) {
            case GF_ROTATION_NONE:
                break;
            case GF_ROTATION_CLOCKWISE_90:
                point = (FxPoint2D){.x = point.y, .y = width - point.x};
                break;
            case GF_ROTATION_180:
                point = (FxPoint2D){.x = width - point.x, .y = height - point.y};
                break;
            case GF_ROTATION_CLOCKWISE_270:
                point = (FxPoint2D){.x = height - point.y, .y = point.x};
                break;
            default:
                return NO;
        }
        left = fmin(left, point.x);
        bottom = fmin(bottom, point.y);
        right = fmax(right, point.x);
        top = fmax(top, point.y);
    }
    return GFRoundCoordinate(left, &outRect->left) &&
           GFRoundCoordinate(bottom, &outRect->bottom) &&
           GFRoundCoordinate(right, &outRect->right) &&
           GFRoundCoordinate(top, &outRect->top);
}

static BOOL GFScaleRect(
    GFRectI32 rect,
    GFDimensionsU32 sourceDimensions,
    GFDimensionsU32 destinationDimensions,
    GFRectI32 *outRect
) {
    if (sourceDimensions.width == 0 || sourceDimensions.height == 0) {
        return NO;
    }
    double scaleX = (double)destinationDimensions.width /
                    (double)sourceDimensions.width;
    double scaleY = (double)destinationDimensions.height /
                    (double)sourceDimensions.height;
    return GFRoundCoordinate((double)rect.left * scaleX, &outRect->left) &&
           GFRoundCoordinate((double)rect.bottom * scaleY, &outRect->bottom) &&
           GFRoundCoordinate((double)rect.right * scaleX, &outRect->right) &&
           GFRoundCoordinate((double)rect.top * scaleY, &outRect->top);
}

static BOOL GFRawTileMatchesTexture(GFFrameGeometryImageSnapshot snapshot) {
    if (!GFPositiveRect(snapshot.tile_pixel_bounds)) {
        return NO;
    }
    int64_t width = (int64_t)snapshot.tile_pixel_bounds.right -
                    (int64_t)snapshot.tile_pixel_bounds.left;
    int64_t height = (int64_t)snapshot.tile_pixel_bounds.top -
                     (int64_t)snapshot.tile_pixel_bounds.bottom;
    return width == snapshot.texture_dimensions.width &&
           height == snapshot.texture_dimensions.height;
}

static GFAdapterAffine GFAbsoluteToNormalized(
    GFRectI32 rect,
    FxImageOrigin origin
) {
    double width = (double)rect.right - (double)rect.left;
    double height = (double)rect.top - (double)rect.bottom;
    if (origin == kFxImageOrigin_TOP_LEFT) {
        return (GFAdapterAffine){.values = {
            1.0 / width, 0.0, -(double)rect.left / width,
            0.0, -1.0 / height, (double)rect.top / height,
            0.0, 0.0, 1.0,
        }};
    }
    return (GFAdapterAffine){.values = {
        1.0 / width, 0.0, -(double)rect.left / width,
        0.0, 1.0 / height, -(double)rect.bottom / height,
        0.0, 0.0, 1.0,
    }};
}

static GFAdapterAffine GFNormalizedToAbsolute(
    GFRectI32 rect,
    FxImageOrigin origin
) {
    double width = (double)rect.right - (double)rect.left;
    double height = (double)rect.top - (double)rect.bottom;
    if (origin == kFxImageOrigin_TOP_LEFT) {
        return (GFAdapterAffine){.values = {
            width, 0.0, rect.left,
            0.0, -height, rect.top,
            0.0, 0.0, 1.0,
        }};
    }
    return (GFAdapterAffine){.values = {
        width, 0.0, rect.left,
        0.0, height, rect.bottom,
        0.0, 0.0, 1.0,
    }};
}

static BOOL GFInvertAffine(
    const GFAdapterAffine *transform,
    GFAdapterAffine *outInverse
) {
    double a = transform->values[0];
    double b = transform->values[1];
    double c = transform->values[3];
    double d = transform->values[4];
    double tx = transform->values[2];
    double ty = transform->values[5];
    double determinant = a * d - b * c;
    if (!isfinite(determinant) || fabs(determinant) <= 1.0e-12) {
        return NO;
    }
    *outInverse = (GFAdapterAffine){.values = {
        d / determinant,
        -b / determinant,
        (b * ty - d * tx) / determinant,
        -c / determinant,
        a / determinant,
        (c * tx - a * ty) / determinant,
        0.0,
        0.0,
        1.0,
    }};
    return YES;
}

BOOL GFFrameGeometryBuild(
    GFFrameGeometryImageSnapshot source,
    GFFrameGeometryImageSnapshot destination,
    GFProjectGeometry project,
    GFFrameGeometry *outGeometry,
    NSError **outError
) {
    if (outGeometry == NULL) {
        return GFFail(NULL, outError, @"Frame geometry output is null");
    }
    *outGeometry = (GFFrameGeometry){0};
    if (outError != NULL) {
        *outError = nil;
    }
    if (source.pixel_transform == nil || source.inverse_pixel_transform == nil ||
        destination.pixel_transform == nil ||
        destination.inverse_pixel_transform == nil) {
        return GFFail(outGeometry, outError, @"FxImageTile pixel transforms are missing");
    }
    if ((source.image_origin != kFxImageOrigin_BOTTOM_LEFT &&
         source.image_origin != kFxImageOrigin_TOP_LEFT) ||
        (destination.image_origin != kFxImageOrigin_BOTTOM_LEFT &&
         destination.image_origin != kFxImageOrigin_TOP_LEFT)) {
        return GFFail(outGeometry, outError, @"FxImageTile image origin is unsupported");
    }
    if (!GFPositiveRect(source.image_pixel_bounds) ||
        !GFPositiveRect(destination.image_pixel_bounds) ||
        !GFRawTileMatchesTexture(source) ||
        !GFRawTileMatchesTexture(destination) ||
        source.texture_dimensions.width == 0 || source.texture_dimensions.height == 0 ||
        source.texture_dimensions.width != destination.texture_dimensions.width ||
        source.texture_dimensions.height != destination.texture_dimensions.height) {
        return GFFail(outGeometry, outError, @"FxImageTile bounds conflict with Metal texture dimensions");
    }
    if (project.reserved != 0 || project.input_dimensions.width == 0 ||
        project.input_dimensions.height == 0 || project.output_dimensions.width == 0 ||
        project.output_dimensions.height == 0) {
        return GFFail(outGeometry, outError, @"Loaded project geometry is invalid");
    }
    if (project.video_rotation != GF_ROTATION_NONE &&
        project.video_rotation != GF_ROTATION_CLOCKWISE_90 &&
        project.video_rotation != GF_ROTATION_180 &&
        project.video_rotation != GF_ROTATION_CLOCKWISE_270) {
        return GFFail(outGeometry, outError, @"Loaded project rotation is unsupported");
    }

    GFAdapterAffine sourceForward = {0};
    GFAdapterAffine sourceInverse = {0};
    GFAdapterAffine destinationForward = {0};
    GFAdapterAffine destinationInverse = {0};
    if (!GFExtractAffine(source.pixel_transform, source.image_pixel_bounds, &sourceForward) ||
        !GFExtractAffine(source.inverse_pixel_transform, source.image_pixel_bounds, &sourceInverse) ||
        !GFExtractAffine(destination.pixel_transform, destination.image_pixel_bounds, &destinationForward) ||
        !GFExtractAffine(destination.inverse_pixel_transform, destination.image_pixel_bounds, &destinationInverse) ||
        !GFScaleTranslate(&sourceForward) || !GFScaleTranslate(&sourceInverse) ||
        !GFScaleTranslate(&destinationForward) || !GFScaleTranslate(&destinationInverse)) {
        return GFFail(outGeometry, outError, @"FxImageTile transform is outside ScaleTranslate support");
    }
    GFAdapterAffine sourcePair = GFMultiply(&sourceForward, &sourceInverse);
    GFAdapterAffine sourcePairReverse = GFMultiply(&sourceInverse, &sourceForward);
    GFAdapterAffine destinationPair = GFMultiply(&destinationForward, &destinationInverse);
    GFAdapterAffine destinationPairReverse =
        GFMultiply(&destinationInverse, &destinationForward);
    if (!GFIdentity(&sourcePair) || !GFIdentity(&sourcePairReverse) ||
        !GFIdentity(&destinationPair) || !GFIdentity(&destinationPairReverse)) {
        return GFFail(outGeometry, outError, @"FxImageTile forward and inverse transforms disagree");
    }

    GFRectI32 sourceImageRect = {0};
    GFRectI32 destinationImageRect = {0};
    GFRectI32 sourceTileRect = {0};
    GFRectI32 destinationTileRect = {0};
    GFDimensionsU32 sourceIdealDimensions = {0};
    GFDimensionsU32 outputDimensions = {0};
    if (!GFTransformRect(&sourceInverse, source.image_pixel_bounds, &sourceImageRect) ||
        !GFTransformRect(&destinationInverse, destination.image_pixel_bounds, &destinationImageRect) ||
        !GFTransformRect(&sourceInverse, source.tile_pixel_bounds, &sourceTileRect) ||
        !GFTransformRect(&destinationInverse, destination.tile_pixel_bounds, &destinationTileRect) ||
        !GFRectDimensions(sourceImageRect, &sourceIdealDimensions) ||
        !GFRectDimensions(destinationImageRect, &outputDimensions)) {
        return GFFail(outGeometry, outError, @"FxImageTile transforms do not produce integral ideal bounds");
    }
    BOOL sourceMatchesInput =
        GFDimensionsEqual(sourceIdealDimensions, project.input_dimensions);
    BOOL sourceMatchesOutput =
        GFDimensionsEqual(sourceIdealDimensions, project.output_dimensions);
    GFRotationDegrees inputRotation = GF_ROTATION_UNKNOWN;
    if (project.video_rotation == GF_ROTATION_NONE &&
        (sourceMatchesInput || sourceMatchesOutput)) {
        inputRotation = GF_ROTATION_NONE;
    } else if ((project.video_rotation == GF_ROTATION_CLOCKWISE_90 ||
                project.video_rotation == GF_ROTATION_CLOCKWISE_270) &&
               sourceMatchesInput && !sourceMatchesOutput &&
               GFDimensionsEqual(outputDimensions, project.output_dimensions) &&
               project.output_dimensions.width == project.input_dimensions.height &&
               project.output_dimensions.height == project.input_dimensions.width) {
        inputRotation = project.video_rotation;
    } else if ((project.video_rotation == GF_ROTATION_CLOCKWISE_90 ||
                project.video_rotation == GF_ROTATION_CLOCKWISE_270) &&
               sourceMatchesOutput && !sourceMatchesInput) {
        inputRotation = GF_ROTATION_NONE;
    } else {
        return GFFail(outGeometry, outError, @"Host source bounds conflict with loaded project dimensions");
    }
    GFRectI32 orientedSourceImageRect = {0};
    GFRectI32 orientedSourceTileRect = {0};
    if (!GFOrientRect(sourceImageRect, sourceImageRect, inputRotation, &orientedSourceImageRect) ||
        !GFOrientRect(sourceTileRect, sourceImageRect, inputRotation, &orientedSourceTileRect)) {
        return GFFail(outGeometry, outError, @"Host source orientation cannot be normalized");
    }
    GFDimensionsU32 orientedSourceDimensions = {0};
    if (!GFRectDimensions(orientedSourceImageRect, &orientedSourceDimensions)) {
        return GFFail(outGeometry, outError, @"Host source rotation conflicts with loaded project output dimensions");
    }
    GFRectI32 scaledSourceImageRect = {0};
    GFRectI32 scaledSourceTileRect = {0};
    if (!GFScaleRect(
            orientedSourceImageRect,
            orientedSourceDimensions,
            project.output_dimensions,
            &scaledSourceImageRect) ||
        !GFScaleRect(
            orientedSourceTileRect,
            orientedSourceDimensions,
            project.output_dimensions,
            &scaledSourceTileRect)) {
        return GFFail(outGeometry, outError, @"Host source bounds cannot be scaled to project output dimensions");
    }
    GFDimensionsU32 scaledSourceDimensions = {0};
    if (!GFRectDimensions(scaledSourceImageRect, &scaledSourceDimensions) ||
        !GFDimensionsEqual(scaledSourceDimensions, project.output_dimensions)) {
        return GFFail(outGeometry, outError, @"Host source rotation conflicts with loaded project output dimensions");
    }
    sourceTileRect = scaledSourceTileRect;

    GFAdapterAffine sourceToNormalized =
        GFAbsoluteToNormalized(sourceTileRect, source.image_origin);
    GFAdapterAffine normalizedToDestination =
        GFNormalizedToAbsolute(destinationTileRect, destination.image_origin);
    GFAdapterAffine forward =
        GFMultiply(&normalizedToDestination, &sourceToNormalized);
    GFAdapterAffine inverse = {0};
    if (!GFInvertAffine(&forward, &inverse)) {
        return GFFail(outGeometry, outError, @"Normalized frame geometry is singular");
    }

    GFFrameGeometry geometry = {
        .version = GF_FRAME_GEOMETRY_VERSION,
        .struct_size = (uint32_t)sizeof(GFFrameGeometry),
        .validity = GF_GEOMETRY_VALIDITY_VALID,
        .support = GF_GEOMETRY_SUPPORT_AFFINE_2D,
        .source_dimensions = project.input_dimensions,
        .oriented_dimensions = project.output_dimensions,
        .tile_dimensions = source.texture_dimensions,
        .output_dimensions = outputDimensions,
        .source_rect = sourceTileRect,
        .destination_rect = destinationTileRect,
        .source_origin = (GFImageOrigin)source.image_origin,
        .destination_origin = (GFImageOrigin)destination.image_origin,
        .input_rotation = inputRotation,
        .video_rotation = project.video_rotation,
        .reserved = {0, 0, 0, 0},
    };
    for (NSUInteger index = 0; index < 9; ++index) {
        geometry.forward_transform.values[index] = forward.values[index];
        geometry.inverse_transform.values[index] = inverse.values[index];
    }
    *outGeometry = geometry;
    return YES;
}
