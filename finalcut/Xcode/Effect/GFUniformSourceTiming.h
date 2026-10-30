// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import <CoreMedia/CoreMedia.h>
#import "GyroflowFinalCut.h"

NS_ASSUME_NONNULL_BEGIN
// This opt-in profile covers normal-speed 60000/1001 input and 30000/1001 conformance.
@interface GFUniformSourceTiming : NSObject
@property(nonatomic, readonly) NSInteger samplingMode;
@property(nonatomic, readonly) NSUInteger frameCount;
@property(nonatomic, readonly) NSString *sourceName;
@property(nonatomic, readonly) NSString *projectHash;
+ (nullable instancetype)timingFromString:(NSString *)string projectHash:(NSString *)hash;
+ (nullable NSString *)prepareURL:(NSURL *)url projectData:(NSData *)projectData
                     projectHash:(NSString *)hash mode:(NSInteger)mode error:(NSError **)error;
- (NSString *)stringWithMode:(NSInteger)mode;
- (NSInteger)domainForHostFrame:(CMTime)frame timelineFrame:(CMTime)timeline
                 inputDuration:(CMTime)duration scale:(GFTime)scale;
- (CMTime)sourceTime:(CMTime)time domain:(NSInteger)domain;
@end
NS_ASSUME_NONNULL_END
