#import "GFRenderDiagnostics.h"

static const NSTimeInterval GFRepeatedStatusSummaryInterval = 30.0;

@interface GFRenderDiagnostics ()
@property(nonatomic, strong) NSLock *lock;
@property(nonatomic, copy) GFRenderDiagnosticsClock clock;
@property(nonatomic, copy) GFRenderDiagnosticsLogger logger;
@property(nonatomic) NSInteger lastPassthroughStatus;
@property(nonatomic, copy) NSString *lastPassthroughReason;
@property(nonatomic) NSTimeInterval lastPassthroughLogTime;
@property(nonatomic) NSUInteger suppressedPassthroughCount;
@property(nonatomic) NSUInteger passthroughLogCount;
@property(nonatomic) NSUInteger deviceEnumerationCount;
@property(nonatomic) NSUInteger commandQueueCreationCount;
@property(nonatomic) NSUInteger pipelineCreationCount;
@property(nonatomic) NSUInteger projectDecodeCount;
@property(nonatomic) NSUInteger projectCacheHitCount;
@property(nonatomic) NSUInteger projectCacheMissCount;
@property(nonatomic) NSUInteger cacheResidentBytes;
@property(nonatomic) NSUInteger cachePeakBytes;
@property(nonatomic) NSUInteger cacheEvictionCount;
@property(nonatomic) NSUInteger cachePurgeCount;
@property(nonatomic) NSUInteger memoryPressurePurgeCount;
@property(nonatomic) NSUInteger pluginStateCallCount;
@property(nonatomic) NSUInteger pluginStateBytes;
@property(nonatomic) uint64_t pluginStateEncodeNanoseconds;
@property(nonatomic) NSUInteger gpuTimeSampleCount;
@property(nonatomic) uint64_t gpuNanoseconds;
@property(nonatomic) NSUInteger processedFrameCount;
@property(nonatomic) NSUInteger passthroughFrameCount;
@property(nonatomic) uint64_t frameNanoseconds;
@end

@implementation GFRenderDiagnostics

- (instancetype)init {
    return [self initWithClock:nil logger:nil];
}

- (instancetype)initWithClock:(GFRenderDiagnosticsClock)clock
                        logger:(GFRenderDiagnosticsLogger)logger {
    self = [super init];
    if (self != nil) {
        self.lock = [[NSLock alloc] init];
        self.clock = clock ?: ^NSTimeInterval {
            return [NSDate timeIntervalSinceReferenceDate];
        };
        self.logger = logger ?: ^(NSString *message) {
            (void)message;
        };
        self.lastPassthroughStatus = NSIntegerMin;
        self.lastPassthroughReason = @"";
    }
    return self;
}

- (void)recordPassthroughStatus:(NSInteger)status reason:(NSString *)reason {
    NSString *message = nil;
    [self.lock lock];
    NSTimeInterval now = self.clock();
    BOOL changed = status != self.lastPassthroughStatus ||
        ![reason isEqualToString:self.lastPassthroughReason];
    if (changed) {
        self.lastPassthroughStatus = status;
        self.lastPassthroughReason = [reason copy];
        self.lastPassthroughLogTime = now;
        self.suppressedPassthroughCount = 0;
        self.passthroughLogCount += 1;
        message = [NSString stringWithFormat:@"safe passthrough status=%ld reason=%@",
                                                  (long)status,
                                                  reason];
    } else {
        self.suppressedPassthroughCount += 1;
        if (now - self.lastPassthroughLogTime >= GFRepeatedStatusSummaryInterval) {
            message = [NSString stringWithFormat:
                @"safe passthrough unchanged status=%ld suppressed_frames=%lu",
                (long)status,
                (unsigned long)self.suppressedPassthroughCount];
            self.lastPassthroughLogTime = now;
            self.suppressedPassthroughCount = 0;
            self.passthroughLogCount += 1;
        }
    }
    [self.lock unlock];
    if (message != nil) {
        self.logger(message);
    }
}

- (void)recordDeviceEnumeration {
    [self.lock lock];
    self.deviceEnumerationCount += 1;
    [self.lock unlock];
}

- (void)recordCommandQueueCreation {
    [self.lock lock];
    self.commandQueueCreationCount += 1;
    [self.lock unlock];
}

- (void)recordPipelineCreation {
    [self.lock lock];
    self.pipelineCreationCount += 1;
    [self.lock unlock];
}

- (void)recordProjectDecode {
    [self.lock lock];
    self.projectDecodeCount += 1;
    [self.lock unlock];
}

- (void)recordProjectCacheHit {
    [self.lock lock];
    self.projectCacheHitCount += 1;
    [self.lock unlock];
}

- (void)recordProjectCacheMiss {
    [self.lock lock];
    self.projectCacheMissCount += 1;
    [self.lock unlock];
}

- (void)recordCacheInsertionBytes:(NSUInteger)bytes {
    [self.lock lock];
    self.cacheResidentBytes += bytes;
    self.cachePeakBytes = MAX(self.cachePeakBytes, self.cacheResidentBytes);
    [self.lock unlock];
}

- (void)recordCacheEvictionBytes:(NSUInteger)bytes {
    [self.lock lock];
    self.cacheResidentBytes = bytes >= self.cacheResidentBytes
        ? 0
        : self.cacheResidentBytes - bytes;
    self.cacheEvictionCount += 1;
    [self.lock unlock];
}

- (void)recordCachePurgeForMemoryPressure:(BOOL)memoryPressure {
    [self.lock lock];
    self.cacheResidentBytes = 0;
    self.cachePurgeCount += 1;
    if (memoryPressure) {
        self.memoryPressurePurgeCount += 1;
    }
    [self.lock unlock];
}

- (void)recordPluginStateBytes:(NSUInteger)bytes encodeSeconds:(NSTimeInterval)seconds {
    [self.lock lock];
    self.pluginStateCallCount += 1;
    self.pluginStateBytes += bytes;
    self.pluginStateEncodeNanoseconds += (uint64_t)(MAX(seconds, 0.0) * 1000000000.0);
    [self.lock unlock];
}

- (void)recordGPUSeconds:(NSTimeInterval)seconds {
    [self.lock lock];
    self.gpuTimeSampleCount += 1;
    self.gpuNanoseconds += (uint64_t)(MAX(seconds, 0.0) * 1000000000.0);
    [self.lock unlock];
}

- (void)recordFrameSeconds:(NSTimeInterval)seconds processed:(BOOL)processed {
    [self.lock lock];
    if (processed) {
        self.processedFrameCount += 1;
    } else {
        self.passthroughFrameCount += 1;
    }
    self.frameNanoseconds += (uint64_t)(MAX(seconds, 0.0) * 1000000000.0);
    [self.lock unlock];
}

- (NSDictionary<NSString *, NSNumber *> *)snapshot {
    [self.lock lock];
    NSDictionary *snapshot = @{
        @"passthrough_logs" : @(self.passthroughLogCount),
        @"device_enumerations" : @(self.deviceEnumerationCount),
        @"command_queue_creations" : @(self.commandQueueCreationCount),
        @"pipeline_creations" : @(self.pipelineCreationCount),
        @"project_decodes" : @(self.projectDecodeCount),
        @"project_cache_hits" : @(self.projectCacheHitCount),
        @"project_cache_misses" : @(self.projectCacheMissCount),
        @"cache_resident_bytes" : @(self.cacheResidentBytes),
        @"cache_peak_bytes" : @(self.cachePeakBytes),
        @"cache_evictions" : @(self.cacheEvictionCount),
        @"cache_purges" : @(self.cachePurgeCount),
        @"memory_pressure_purges" : @(self.memoryPressurePurgeCount),
        @"plugin_state_calls" : @(self.pluginStateCallCount),
        @"plugin_state_bytes" : @(self.pluginStateBytes),
        @"plugin_state_encode_ns" : @(self.pluginStateEncodeNanoseconds),
        @"gpu_time_samples" : @(self.gpuTimeSampleCount),
        @"gpu_ns" : @(self.gpuNanoseconds),
        @"processed_frames" : @(self.processedFrameCount),
        @"passthrough_frames" : @(self.passthroughFrameCount),
        @"frame_ns" : @(self.frameNanoseconds),
    };
    [self.lock unlock];
    return snapshot;
}

@end
