#import "GFRenderCache.h"

static NSString *const GFRenderCacheErrorDomain =
    @"com.niyien.gyroflow.finalcut.render-cache";

static NSString *GFCacheBridgeError(const GFError *error, NSString *fallback) {
    if (error == NULL || error->message == NULL) {
        return fallback;
    }
    NSString *message = [NSString stringWithUTF8String:error->message];
    return message ?: fallback;
}

@interface GFRenderSnapshot ()
@property(nonatomic, readwrite) GFFinalCutInstance *instance;
@property(nonatomic, readwrite) GFRenderState *state;
@property(nonatomic, readwrite) GFStatus preparationStatus;
@property(nonatomic, readwrite) NSString *statusMessage;
@end

@implementation GFRenderSnapshot

- (void)dealloc {
    gf_finalcut_instance_free(self.instance);
}

@end

@interface GFRenderCache ()
@property(nonatomic, strong) NSCache<NSData *, GFRenderSnapshot *> *snapshots;
@end

@implementation GFRenderCache

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.snapshots = [[NSCache alloc] init];
        self.snapshots.countLimit = 32;
    }
    return self;
}

- (GFRenderSnapshot *)buildSnapshotForState:(GFRenderState *)state {
    GFRenderSnapshot *snapshot = [[GFRenderSnapshot alloc] init];
    snapshot.state = state;
    snapshot.preparationStatus = GF_STATUS_OK;
    snapshot.statusMessage = @"Ready";

    GFError *bridgeError = NULL;
    snapshot.instance = gf_finalcut_instance_create(&bridgeError);
    if (snapshot.instance == NULL) {
        snapshot.preparationStatus = GF_STATUS_PANIC;
        snapshot.statusMessage =
            GFCacheBridgeError(bridgeError, @"unable to create render snapshot");
        gf_finalcut_error_free(bridgeError);
        return snapshot;
    }
    NSData *projectPayload = [state.projectPayload dataUsingEncoding:NSASCIIStringEncoding];
    GFStatus status = state.projectPayload.length == 0
        ? GF_STATUS_INVALID_PROJECT
        : gf_finalcut_instance_load_project_payload(
              snapshot.instance,
              projectPayload.bytes,
              projectPayload.length,
              &bridgeError
          );
    if (status != GF_STATUS_OK) {
        snapshot.preparationStatus = status;
        snapshot.statusMessage = state.projectPayload.length == 0
            ? @"Import Gyroflow Project before rendering"
            : GFCacheBridgeError(bridgeError, @"invalid embedded project payload");
        gf_finalcut_error_free(bridgeError);
        return snapshot;
    }
    gf_finalcut_error_free(bridgeError);
    bridgeError = NULL;

    GFTimeRange effectBounds = state.effectBounds;
    GFTimeRange inputBounds = state.inputBounds;
    if (state.timingPayload.length > 0) {
        NSData *timingPayload =
            [state.timingPayload dataUsingEncoding:NSASCIIStringEncoding];
        status = gf_finalcut_instance_load_timing_payload(
            snapshot.instance,
            timingPayload.bytes,
            timingPayload.length,
            &effectBounds,
            &inputBounds,
            &bridgeError
        );
        if (status != GF_STATUS_OK) {
            snapshot.preparationStatus = status;
            snapshot.statusMessage =
                GFCacheBridgeError(bridgeError, @"Reprocess Project Required");
            gf_finalcut_error_free(bridgeError);
            return snapshot;
        }
        gf_finalcut_error_free(bridgeError);
        bridgeError = NULL;
    } else {
        snapshot.statusMessage = @"Direct stabilization ready";
    }

    GFRenderParameters parameters = state.parameters;
    status = gf_finalcut_instance_set_render_parameters(
        snapshot.instance,
        &parameters,
        &bridgeError
    );
    if (status != GF_STATUS_OK) {
        snapshot.preparationStatus = status;
        snapshot.statusMessage =
            GFCacheBridgeError(bridgeError, @"invalid render parameters");
    }
    gf_finalcut_error_free(bridgeError);
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
    [self.snapshots setObject:snapshot forKey:[pluginStateData copy]];
    return snapshot;
}

- (void)discardAllSnapshots {
    [self.snapshots removeAllObjects];
}

@end
