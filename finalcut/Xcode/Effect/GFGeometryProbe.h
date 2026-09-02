// SPDX-License-Identifier: GPL-3.0-or-later

#import <FxPlug/FxPlugSDK.h>
#import <Metal/Metal.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT void GFGeometryProbeRecordFrame(
    FxImageTile *sourceImage,
    FxImageTile *destinationImage,
    id<MTLTexture> sourceTexture,
    id<MTLTexture> destinationTexture,
    CMTime renderTime
);

NS_ASSUME_NONNULL_END
