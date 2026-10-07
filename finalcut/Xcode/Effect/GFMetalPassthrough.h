// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import "GyroflowFinalCut.h"

@interface GFMetalPassthrough : NSObject
- (instancetype)initWithDevice:(id<MTLDevice>)device;
- (BOOL)copySource:(id<MTLTexture>)source
       sourceImage:(GFHostImageV2)sourceImage
      destination:(id<MTLTexture>)destination
 destinationImage:(GFHostImageV2)destinationImage
     commandQueue:(id<MTLCommandQueue>)queue
       gpuSeconds:(double *)gpuSeconds
            error:(NSError **)error;
@end
