#import "GFRenderState.h"
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface GFRenderSnapshot : NSObject

@property(nonatomic, readonly) GFFinalCutInstance *instance;
@property(nonatomic, readonly) GFRenderState *state;
@property(nonatomic, readonly) GFStatus preparationStatus;
@property(nonatomic, readonly) NSString *statusMessage;

@end

@interface GFRenderCache : NSObject

- (nullable GFRenderSnapshot *)snapshotForPluginStateData:(NSData *)pluginStateData
                                                    error:(NSError **)error;
- (void)discardAllSnapshots;

@end

NS_ASSUME_NONNULL_END
