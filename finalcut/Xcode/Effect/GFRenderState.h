#import "GyroflowFinalCut.h"
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface GFRenderState : NSObject <NSSecureCoding, NSCopying>

@property(nonatomic, readonly) NSString *projectPayload;
@property(nonatomic, readonly) NSString *timingPayload;
@property(nonatomic, readonly) GFRenderParameters parameters;
@property(nonatomic, readonly) GFTimeRange effectBounds;
@property(nonatomic, readonly) GFTimeRange inputBounds;

- (instancetype)initWithProjectPayload:(NSString *)projectPayload
                         timingPayload:(NSString *)timingPayload
                            parameters:(GFRenderParameters)parameters
                          effectBounds:(GFTimeRange)effectBounds
                           inputBounds:(GFTimeRange)inputBounds;

@end

NS_ASSUME_NONNULL_END
