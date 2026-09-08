#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

typedef NSTimeInterval (^GFRenderDiagnosticsClock)(void);
typedef void (^GFRenderDiagnosticsLogger)(NSString *message);

@interface GFRenderDiagnostics : NSObject

- (instancetype)initWithClock:(nullable GFRenderDiagnosticsClock)clock
                        logger:(nullable GFRenderDiagnosticsLogger)logger
    NS_DESIGNATED_INITIALIZER;
- (instancetype)init;

- (void)recordPassthroughStatus:(NSInteger)status reason:(NSString *)reason;
- (void)recordDeviceEnumeration;
- (void)recordCommandQueueCreation;
- (void)recordPipelineCreation;
- (void)recordProjectDecode;
- (void)recordProjectCacheHit;
- (void)recordProjectCacheMiss;
- (void)recordCacheInsertionBytes:(NSUInteger)bytes;
- (void)recordCacheEvictionBytes:(NSUInteger)bytes;
- (void)recordCachePurgeForMemoryPressure:(BOOL)memoryPressure;
- (void)recordPluginStateBytes:(NSUInteger)bytes encodeSeconds:(NSTimeInterval)seconds;
- (void)recordGPUSeconds:(NSTimeInterval)seconds;
- (void)recordFrameSeconds:(NSTimeInterval)seconds processed:(BOOL)processed;
- (NSDictionary<NSString *, NSNumber *> *)snapshot;

@end

NS_ASSUME_NONNULL_END
