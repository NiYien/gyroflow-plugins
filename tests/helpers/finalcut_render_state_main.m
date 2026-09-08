#import <Foundation/Foundation.h>
#import <stdatomic.h>
#import <string.h>

#import "GFRenderCache.h"

struct GFFinalCutInstance {
    int marker;
};

static _Atomic(int) gCreateCount = 0;
static _Atomic(int) gTimingLoadCount = 0;

GFFinalCutInstance *gf_finalcut_instance_create(GFError **out_error) {
    if (out_error != NULL) {
        *out_error = NULL;
    }
    atomic_fetch_add_explicit(&gCreateCount, 1, memory_order_relaxed);
    GFFinalCutInstance *instance = calloc(1, sizeof(GFFinalCutInstance));
    instance->marker = 42;
    return instance;
}

void gf_finalcut_instance_free(GFFinalCutInstance *instance) {
    free(instance);
}

GFStatus gf_finalcut_instance_load_project_payload(
    GFFinalCutInstance *instance,
    const uint8_t *payload_bytes,
    size_t payload_len,
    GFError **out_error
) {
    (void)out_error;
    return instance != NULL && payload_bytes != NULL && payload_len > 0
        ? GF_STATUS_OK
        : GF_STATUS_INVALID_PROJECT;
}

GFStatus gf_finalcut_instance_load_timing_payload(
    GFFinalCutInstance *instance,
    const uint8_t *payload_bytes,
    size_t payload_len,
    const GFTimeRange *observed_effect_bounds,
    const GFTimeRange *observed_input_bounds,
    GFError **out_error
) {
    (void)observed_effect_bounds;
    (void)observed_input_bounds;
    (void)out_error;
    atomic_fetch_add_explicit(&gTimingLoadCount, 1, memory_order_relaxed);
    static const char invalidTiming[] = "invalid-timing";
    if (payload_len == sizeof(invalidTiming) - 1 &&
        memcmp(payload_bytes, invalidTiming, payload_len) == 0) {
        return GF_STATUS_MISSING_TIMING;
    }
    return instance != NULL && payload_bytes != NULL && payload_len > 0
        ? GF_STATUS_OK
        : GF_STATUS_MISSING_TIMING;
}

GFStatus gf_finalcut_instance_set_render_parameters(
    GFFinalCutInstance *instance,
    const GFRenderParameters *parameters,
    GFError **out_error
) {
    (void)out_error;
    return instance != NULL && parameters != NULL
        ? GF_STATUS_OK
        : GF_STATUS_INVALID_ARGUMENT;
}

void gf_finalcut_error_free(GFError *error) {
    (void)error;
}

int main(void) {
    @autoreleasepool {
        GFRenderParameters parameters = {
            .fov = 1.25,
            .smoothness = 42.0,
            .lens_correction = 80.0,
            .horizon_lock_amount = 30.0,
            .horizon_lock_roll = 5.0,
            .zoom_mode = 2,
            .overview = 1,
            .reserved = {0, 0, 0},
        };
        GFTimeRange effectBounds = {
            .start = {.numerator = 1, .denominator = 24},
            .duration = {.numerator = 10, .denominator = 1},
        };
        GFTimeRange inputBounds = {
            .start = {.numerator = 5, .denominator = 1},
            .duration = {.numerator = 10, .denominator = 1},
        };
        GFRenderState *state =
            [[GFRenderState alloc] initWithProjectPayload:@"project-payload"
                                      projectDisplayName:@"A001.gyroflow"
                                      projectContentHash:@"fixture-hash"
                                          timingPayload:@"timing-payload"
                                                   mode:GFRenderModeRouteD
                                             parameters:parameters
                                           effectBounds:effectBounds
                                            inputBounds:inputBounds];
        NSError *error = nil;
        NSData *data = [NSKeyedArchiver archivedDataWithRootObject:state
                                            requiringSecureCoding:YES
                                                            error:&error];
        if (data == nil) {
            return 2;
        }
        GFRenderState *roundTrip = [NSKeyedUnarchiver
            unarchivedObjectOfClass:[GFRenderState class]
                           fromData:data
                              error:&error];
        if (roundTrip == nil) {
            return 3;
        }
        GFRenderDiagnostics *diagnostics = [[GFRenderDiagnostics alloc] init];
        GFRenderCache *cache = [[GFRenderCache alloc] initWithDiagnostics:diagnostics];
        GFRenderSnapshot *first = [cache snapshotForPluginStateData:data error:&error];
        GFRenderSnapshot *second = [cache snapshotForPluginStateData:data error:&error];
        BOOL sameSnapshot = first == second;
        GFRenderState *hashConflictState = [[GFRenderState alloc]
            initWithProjectPayload:@"different-project-payload"
               projectDisplayName:@"Conflict.gyroflow"
               projectContentHash:@"fixture-hash"
                   timingPayload:@"timing-payload"
                            mode:GFRenderModeRouteD
                      parameters:parameters
                    effectBounds:effectBounds
                     inputBounds:inputBounds];
        NSData *hashConflictData = [NSKeyedArchiver
            archivedDataWithRootObject:hashConflictState
                 requiringSecureCoding:YES
                                 error:&error];
        GFRenderSnapshot *hashConflictSnapshot =
            [cache snapshotForPluginStateData:hashConflictData error:&error];
        BOOL hashConflictRejected = hashConflictSnapshot != nil &&
            hashConflictSnapshot.preparationStatus == GF_STATUS_INVALID_PROJECT &&
            hashConflictSnapshot.instance == NULL;
        BOOL keyframesReusePreparedProject = YES;
        for (NSUInteger frame = 0; frame < 300; ++frame) {
            GFRenderParameters frameParameters = parameters;
            frameParameters.fov = 1.0 + (double)frame / 1000.0;
            GFRenderState *frameState = [[GFRenderState alloc]
                initWithProjectPayload:@"project-payload"
                   projectDisplayName:@"A001.gyroflow"
                   projectContentHash:@"fixture-hash"
                       timingPayload:@"timing-payload"
                                mode:GFRenderModeRouteD
                          parameters:frameParameters
                        effectBounds:effectBounds
                         inputBounds:inputBounds];
            NSData *frameData = [NSKeyedArchiver archivedDataWithRootObject:frameState
                                                      requiringSecureCoding:YES
                                                                      error:&error];
            GFRenderSnapshot *frameSnapshot =
                [cache snapshotForPluginStateData:frameData error:&error];
            keyframesReusePreparedProject = keyframesReusePreparedProject &&
                frameSnapshot.instance == first.instance &&
                frameSnapshot.renderLock == first.renderLock;
        }
        NSDictionary *keyframeMetrics = [diagnostics snapshot];
        [cache discardAllSnapshots];
        GFRenderSnapshot *rebuiltAfterPurge =
            [cache snapshotForPluginStateData:data error:&error];
        BOOL evictionReconstructed = rebuiltAfterPurge != nil &&
            rebuiltAfterPurge.preparationStatus == GF_STATUS_OK &&
            rebuiltAfterPurge.instance != first.instance;

        GFRenderCache *restartedCache = [[GFRenderCache alloc]
            initWithDiagnostics:[[GFRenderDiagnostics alloc] init]];
        GFRenderSnapshot *restartSnapshot =
            [restartedCache snapshotForPluginStateData:data error:&error];
        BOOL xpcRestartReconstructed = restartSnapshot != nil &&
            restartSnapshot.preparationStatus == GF_STATUS_OK &&
            restartSnapshot.instance != first.instance;

        GFRenderState *secondInstanceState = [[GFRenderState alloc]
            initWithProjectPayload:@"second-project-payload"
               projectDisplayName:@"B002.gyroflow"
               projectContentHash:@"second-fixture-hash"
                   timingPayload:@"timing-payload"
                            mode:GFRenderModeRouteD
                      parameters:parameters
                    effectBounds:effectBounds
                     inputBounds:inputBounds];
        NSData *secondInstanceData = [NSKeyedArchiver
            archivedDataWithRootObject:secondInstanceState
                 requiringSecureCoding:YES
                                 error:&error];
        GFRenderSnapshot *secondInstanceSnapshot =
            [cache snapshotForPluginStateData:secondInstanceData error:&error];
        BOOL multipleInstancesIndependent = secondInstanceSnapshot != nil &&
            secondInstanceSnapshot.preparationStatus == GF_STATUS_OK &&
            secondInstanceSnapshot.instance != rebuiltAfterPurge.instance &&
            secondInstanceSnapshot.renderLock != rebuiltAfterPurge.renderLock;

        NSString *largePayload = [@"large-project-" stringByPaddingToLength:1024 * 1024
                                                                 withString:@"abcdef"
                                                            startingAtIndex:0];
        GFRenderState *largeState = [[GFRenderState alloc]
            initWithProjectPayload:largePayload
               projectDisplayName:@"large.gyroflow"
               projectContentHash:@"large-fixture-hash"
                   timingPayload:@"timing-payload"
                            mode:GFRenderModeRouteD
                      parameters:parameters
                    effectBounds:effectBounds
                     inputBounds:inputBounds];
        NSData *largeData = [NSKeyedArchiver archivedDataWithRootObject:largeState
                                                  requiringSecureCoding:YES
                                                                  error:&error];
        GFRenderSnapshot *largeSnapshot =
            [cache snapshotForPluginStateData:largeData error:&error];
        BOOL largeProjectReady = largeSnapshot != nil &&
            largeSnapshot.preparationStatus == GF_STATUS_OK;

        GFRenderState *directState =
            [[GFRenderState alloc] initWithProjectPayload:@"project-payload"
                                           timingPayload:@""
                                              parameters:parameters
                                            effectBounds:effectBounds
                                             inputBounds:inputBounds];
        NSData *directData = [NSKeyedArchiver archivedDataWithRootObject:directState
                                                   requiringSecureCoding:YES
                                                                   error:&error];
        int directLoadCountBefore =
            atomic_load_explicit(&gTimingLoadCount, memory_order_relaxed);
        GFRenderSnapshot *directSnapshot =
            [cache snapshotForPluginStateData:directData error:&error];
        int directLoadCountAfter =
            atomic_load_explicit(&gTimingLoadCount, memory_order_relaxed);

        GFRenderState *invalidState =
            [[GFRenderState alloc] initWithProjectPayload:@"project-payload"
                                           timingPayload:@"invalid-timing"
                                              parameters:parameters
                                            effectBounds:effectBounds
                                             inputBounds:inputBounds];
        NSData *invalidData = [NSKeyedArchiver archivedDataWithRootObject:invalidState
                                                    requiringSecureCoding:YES
                                                                    error:&error];
        GFRenderSnapshot *invalidSnapshot =
            [cache snapshotForPluginStateData:invalidData error:&error];

        __block _Atomic(int) failures = 0;
        dispatch_apply(100, dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^(size_t index) {
            @autoreleasepool {
                (void)index;
                NSError *snapshotError = nil;
                GFRenderSnapshot *snapshot =
                    [cache snapshotForPluginStateData:data error:&snapshotError];
                if (snapshot == nil ||
                    snapshot.preparationStatus != GF_STATUS_OK ||
                    snapshot.state.parameters.fov != 1.25) {
                    atomic_fetch_add_explicit(&failures, 1, memory_order_relaxed);
                }
            }
        });
        GFRenderSnapshot *corrupt =
            [cache snapshotForPluginStateData:[@"not-an-archive"
                dataUsingEncoding:NSUTF8StringEncoding]
                                       error:&error];
        NSDictionary *result = @{
            @"secureRoundTrip" : @(
                roundTrip.schemaVersion == 2 &&
                [roundTrip.projectPayload isEqualToString:@"project-payload"] &&
                [roundTrip.projectDisplayName isEqualToString:@"A001.gyroflow"] &&
                [roundTrip.projectContentHash isEqualToString:@"fixture-hash"] &&
                roundTrip.mode == GFRenderModeRouteD &&
                roundTrip.parameters.zoom_mode == 2 &&
                roundTrip.effectBounds.start.denominator == 24
            ),
            @"sameSnapshot" : @(sameSnapshot),
            @"hashConflictRejected" : @(hashConflictRejected),
            @"keyframesReusePreparedProject" : @(keyframesReusePreparedProject),
            @"keyframeProjectDecodes" : keyframeMetrics[@"project_decodes"],
            @"evictionReconstructed" : @(evictionReconstructed),
            @"xpcRestartReconstructed" : @(xpcRestartReconstructed),
            @"multipleInstancesIndependent" : @(multipleInstancesIndependent),
            @"largeProjectReady" : @(largeProjectReady),
            @"directSnapshotReady" : @(
                directSnapshot != nil &&
                directSnapshot.preparationStatus == GF_STATUS_OK
            ),
            @"directSkippedTimingLoader" : @(
                directLoadCountBefore == directLoadCountAfter
            ),
            @"invalidTimingRejected" : @(
                invalidSnapshot != nil &&
                invalidSnapshot.preparationStatus == GF_STATUS_MISSING_TIMING
            ),
            @"concurrentFailures" : @(atomic_load_explicit(&failures, memory_order_relaxed)),
            @"createCount" : @(atomic_load_explicit(&gCreateCount, memory_order_relaxed)),
            @"corruptRejected" : @(corrupt == nil),
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
