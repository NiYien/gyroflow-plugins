#import "GFRenderState.h"
#import "GFRenderDiagnostics.h"
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface GFRenderSnapshot : NSObject

@property(nonatomic, readonly) GFFinalCutInstance *instance;
@property(nonatomic, readonly) GFRenderState *state;
@property(nonatomic, readonly) GFStatus preparationStatus;
@property(nonatomic, readonly) NSString *statusMessage;
@property(nonatomic, readonly) NSLock *renderLock;

@end

@interface GFRenderCache : NSObject

- (instancetype)initWithDiagnostics:(GFRenderDiagnostics *)diagnostics
    NS_DESIGNATED_INITIALIZER;
- (instancetype)init;

- (nullable GFRenderSnapshot *)snapshotForPluginStateData:(NSData *)pluginStateData
                                                    error:(NSError **)error;
- (void)discardAllSnapshots;

@end

NS_ASSUME_NONNULL_END
