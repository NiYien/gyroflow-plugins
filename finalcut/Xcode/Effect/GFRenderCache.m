#import "GFRenderCache.h"

static NSString *const GFRenderCacheErrorDomain =
    @"com.niyien.gyroflow.finalcut.render-cache";
static const NSUInteger GFRenderSnapshotCountLimit = 32;
static const NSUInteger GFPreparedProjectCountLimit = 8;
static const NSUInteger GFRenderCacheCostLimit = 64 * 1024 * 1024;

@protocol GFRenderCacheCosting <NSObject>
@property(nonatomic, readonly) NSUInteger cacheCost;
@end

static NSString *GFCacheBridgeError(const GFError *error, NSString *fallback) {
    if (error == NULL || error->message == NULL) {
        return fallback;
    }
    NSString *message = [NSString stringWithUTF8String:error->message];
    return message ?: fallback;
}

@interface GFPreparedRenderProject : NSObject <GFRenderCacheCosting>
@property(nonatomic) GFFinalCutInstance *instance;
@property(nonatomic) GFStatus preparationStatus;
@property(nonatomic, copy) NSString *statusMessage;
@property(nonatomic, copy) NSString *projectPayloadIdentity;
@property(nonatomic, strong) NSLock *renderLock;
@property(nonatomic) NSUInteger cacheCost;
@end

@implementation GFPreparedRenderProject

- (void)dealloc {
    gf_finalcut_instance_free(self.instance);
}

@end

@interface GFRenderSnapshot () <GFRenderCacheCosting>
@property(nonatomic, readwrite) GFFinalCutInstance *instance;
@property(nonatomic, readwrite) GFRenderState *state;
@property(nonatomic, readwrite) GFStatus preparationStatus;
@property(nonatomic, readwrite) NSString *statusMessage;
@property(nonatomic, readwrite) NSLock *renderLock;
@property(nonatomic, strong) GFPreparedRenderProject *preparedProject;
@property(nonatomic) NSUInteger cacheCost;
@end

@implementation GFRenderSnapshot
@end

@interface GFRenderCacheEvictionDelegate : NSObject <NSCacheDelegate>
@property(nonatomic, strong) GFRenderDiagnostics *diagnostics;
@end

@implementation GFRenderCacheEvictionDelegate

- (void)cache:(NSCache *)cache willEvictObject:(id)object {
    (void)cache;
    if ([object conformsToProtocol:@protocol(GFRenderCacheCosting)]) {
        [self.diagnostics
            recordCacheEvictionBytes:((id<GFRenderCacheCosting>)object).cacheCost];
    }
}

@end

@interface GFRenderCache ()
@property(nonatomic, strong) NSCache<NSData *, GFRenderSnapshot *> *snapshots;
@property(nonatomic, strong) NSCache<NSString *, GFPreparedRenderProject *> *preparedProjects;
@property(nonatomic, strong) GFRenderDiagnostics *diagnostics;
@property(nonatomic, strong) GFRenderCacheEvictionDelegate *snapshotEvictionDelegate;
@property(nonatomic, strong) GFRenderCacheEvictionDelegate *projectEvictionDelegate;
@property(nonatomic, strong) dispatch_source_t memoryPressureSource;
- (void)discardAllSnapshotsForMemoryPressure:(BOOL)memoryPressure;
@end

@implementation GFRenderCache

- (instancetype)init {
    return [self initWithDiagnostics:[[GFRenderDiagnostics alloc] init]];
}

- (instancetype)initWithDiagnostics:(GFRenderDiagnostics *)diagnostics {
    self = [super init];
    if (self != nil) {
        self.diagnostics = diagnostics;
        self.snapshots = [[NSCache alloc] init];
        self.snapshots.countLimit = GFRenderSnapshotCountLimit;
        self.snapshots.totalCostLimit = GFRenderCacheCostLimit;
        self.snapshotEvictionDelegate = [[GFRenderCacheEvictionDelegate alloc] init];
        self.snapshotEvictionDelegate.diagnostics = diagnostics;
        self.snapshots.delegate = self.snapshotEvictionDelegate;
        self.preparedProjects = [[NSCache alloc] init];
        self.preparedProjects.countLimit = GFPreparedProjectCountLimit;
        self.preparedProjects.totalCostLimit = GFRenderCacheCostLimit;
        self.projectEvictionDelegate = [[GFRenderCacheEvictionDelegate alloc] init];
        self.projectEvictionDelegate.diagnostics = diagnostics;
        self.preparedProjects.delegate = self.projectEvictionDelegate;
        self.memoryPressureSource = dispatch_source_create(
            DISPATCH_SOURCE_TYPE_MEMORYPRESSURE,
            0,
            DISPATCH_MEMORYPRESSURE_WARN | DISPATCH_MEMORYPRESSURE_CRITICAL,
            dispatch_get_global_queue(QOS_CLASS_UTILITY, 0));
        if (self.memoryPressureSource != nil) {
            __weak GFRenderCache *weakSelf = self;
            dispatch_source_set_event_handler(self.memoryPressureSource, ^{
                [weakSelf discardAllSnapshotsForMemoryPressure:YES];
            });
            dispatch_resume(self.memoryPressureSource);
        }
    }
    return self;
}

- (void)dealloc {
    self.snapshots.delegate = nil;
    self.preparedProjects.delegate = nil;
    if (self.memoryPressureSource != nil) {
        dispatch_source_cancel(self.memoryPressureSource);
    }
}

- (NSString *)immutableIdentityForState:(GFRenderState *)state {
    GFTimeRange effect = state.effectBounds;
    GFTimeRange input = state.inputBounds;
    return [NSString stringWithFormat:
        @"%ld|%@|%@|%lld/%lld/%lld/%lld|%lld/%lld/%lld/%lld",
        (long)state.schemaVersion,
        state.projectContentHash,
        state.timingPayload,
        (long long)effect.start.numerator,
        (long long)effect.start.denominator,
        (long long)effect.duration.numerator,
        (long long)effect.duration.denominator,
        (long long)input.start.numerator,
        (long long)input.start.denominator,
        (long long)input.duration.numerator,
        (long long)input.duration.denominator];
}

- (GFPreparedRenderProject *)buildPreparedProjectForState:(GFRenderState *)state {
    GFPreparedRenderProject *prepared = [[GFPreparedRenderProject alloc] init];
    prepared.preparationStatus = GF_STATUS_OK;
    prepared.statusMessage = @"Ready";
    prepared.projectPayloadIdentity = state.projectPayload;
    prepared.renderLock = [[NSLock alloc] init];

    GFError *bridgeError = NULL;
    prepared.instance = gf_finalcut_instance_create(&bridgeError);
    if (prepared.instance == NULL) {
        prepared.preparationStatus = GF_STATUS_PANIC;
        prepared.statusMessage =
            GFCacheBridgeError(bridgeError, @"unable to create render snapshot");
        gf_finalcut_error_free(bridgeError);
        return prepared;
    }
    NSData *projectPayload = [state.projectPayload dataUsingEncoding:NSASCIIStringEncoding];
    [self.diagnostics recordProjectDecode];
    GFStatus status = state.projectPayload.length == 0
        ? GF_STATUS_INVALID_PROJECT
        : gf_finalcut_instance_load_project_payload(
              prepared.instance,
              projectPayload.bytes,
              projectPayload.length,
              &bridgeError
          );
    if (status != GF_STATUS_OK) {
        prepared.preparationStatus = status;
        prepared.statusMessage = state.projectPayload.length == 0
            ? @"Import Gyroflow Project before rendering"
            : GFCacheBridgeError(bridgeError, @"invalid embedded project payload");
        gf_finalcut_error_free(bridgeError);
        return prepared;
    }
    gf_finalcut_error_free(bridgeError);
    bridgeError = NULL;

    GFTimeRange effectBounds = state.effectBounds;
    GFTimeRange inputBounds = state.inputBounds;
    if (state.timingPayload.length > 0) {
        NSData *timingPayload =
            [state.timingPayload dataUsingEncoding:NSASCIIStringEncoding];
        status = gf_finalcut_instance_load_timing_payload(
            prepared.instance,
            timingPayload.bytes,
            timingPayload.length,
            &effectBounds,
            &inputBounds,
            &bridgeError
        );
        if (status != GF_STATUS_OK) {
            prepared.preparationStatus = status;
            prepared.statusMessage =
                GFCacheBridgeError(bridgeError, @"Reprocess Project Required");
            gf_finalcut_error_free(bridgeError);
            return prepared;
        }
        gf_finalcut_error_free(bridgeError);
    } else {
        prepared.statusMessage = @"Direct stabilization ready";
    }
    return prepared;
}

- (GFRenderSnapshot *)buildSnapshotForState:(GFRenderState *)state {
    NSString *identity = [self immutableIdentityForState:state];
    GFPreparedRenderProject *prepared = [self.preparedProjects objectForKey:identity];
    if (prepared == nil) {
        [self.diagnostics recordProjectCacheMiss];
        prepared = [self buildPreparedProjectForState:state];
        NSUInteger cost =
            [state.projectPayload lengthOfBytesUsingEncoding:NSUTF8StringEncoding] +
            [state.timingPayload lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
        prepared.cacheCost = MAX(cost, 1);
        [self.preparedProjects setObject:prepared forKey:identity cost:MAX(cost, 1)];
        [self.diagnostics recordCacheInsertionBytes:prepared.cacheCost];
    } else {
        if (![prepared.projectPayloadIdentity isEqualToString:state.projectPayload]) {
            [self.diagnostics recordProjectCacheMiss];
            GFPreparedRenderProject *conflict = [[GFPreparedRenderProject alloc] init];
            conflict.preparationStatus = GF_STATUS_INVALID_PROJECT;
            conflict.statusMessage = @"Project content conflicts with its persisted hash";
            conflict.projectPayloadIdentity = state.projectPayload;
            conflict.renderLock = [[NSLock alloc] init];
            prepared = conflict;
        } else {
            [self.diagnostics recordProjectCacheHit];
        }
    }

    GFRenderSnapshot *snapshot = [[GFRenderSnapshot alloc] init];
    snapshot.preparedProject = prepared;
    snapshot.instance = prepared.instance;
    snapshot.state = state;
    snapshot.preparationStatus = prepared.preparationStatus;
    snapshot.statusMessage = prepared.statusMessage;
    snapshot.renderLock = prepared.renderLock;
    return snapshot;
}

- (nullable GFRenderSnapshot *)snapshotForPluginStateData:(NSData *)pluginStateData
                                                    error:(NSError **)error {
    if (pluginStateData.length == 0) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFRenderCacheErrorDomain
                                         code:1
                                     userInfo:@{
                                         NSLocalizedDescriptionKey :
                                             @"immutable plugin state is missing"
                                     }];
        }
        return nil;
    }
    @synchronized(self) {
        GFRenderSnapshot *cached = [self.snapshots objectForKey:pluginStateData];
        if (cached != nil) {
            return cached;
        }
        NSError *decodeError = nil;
        GFRenderState *state = [NSKeyedUnarchiver
            unarchivedObjectOfClass:[GFRenderState class]
                           fromData:pluginStateData
                              error:&decodeError];
        if (state == nil) {
            if (error != NULL) {
                *error = [NSError errorWithDomain:GFRenderCacheErrorDomain
                                             code:2
                                         userInfo:@{
                                             NSLocalizedDescriptionKey :
                                                 decodeError.localizedDescription
                                                     ?: @"immutable plugin state is invalid"
                                         }];
            }
            return nil;
        }
        GFRenderSnapshot *snapshot = [self buildSnapshotForState:state];
        snapshot.cacheCost = MAX(pluginStateData.length, 1);
        [self.snapshots setObject:snapshot
                           forKey:[pluginStateData copy]
                             cost:snapshot.cacheCost];
        [self.diagnostics recordCacheInsertionBytes:snapshot.cacheCost];
        return snapshot;
    }
}

- (void)discardAllSnapshots {
    [self discardAllSnapshotsForMemoryPressure:NO];
}

- (void)discardAllSnapshotsForMemoryPressure:(BOOL)memoryPressure {
    @synchronized(self) {
        [self.snapshots removeAllObjects];
        [self.preparedProjects removeAllObjects];
        [self.diagnostics recordCachePurgeForMemoryPressure:memoryPressure];
    }
}

@end
