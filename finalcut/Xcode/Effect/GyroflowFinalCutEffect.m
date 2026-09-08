#import "GyroflowFinalCutEffect.h"

#import "GFFrameGeometryAdapter.h"
#import "GFGeometryProbe.h"
#import "GFLocalization.h"
#import "GFParameterCommitter.h"
#import "GFParameterIDs.h"
#import "GFProjectDropView.h"
#import "GFProjectStore.h"
#import "GFRenderCache.h"
#import "GFRenderDiagnostics.h"
#import "GFRenderPolicy.h"
#import "GyroflowFinalCut.h"
#import <Metal/Metal.h>
#import <os/log.h>

static NSError *GFFxError(NSString *message) {
    return [NSError errorWithDomain:FxPlugErrorDomain
                               code:(kFxError_ThirdPartyDeveloperStart + 1)
                           userInfo:@{NSLocalizedDescriptionKey : message}];
}

static NSString *GFBridgeErrorMessage(const GFError *error, NSString *fallback) {
    if (error == NULL || error->message == NULL) {
        return fallback;
    }
    NSString *message = [NSString stringWithUTF8String:error->message];
    return message ?: fallback;
}

static BOOL GFColorSpacesEqual(CGColorSpaceRef first, CGColorSpaceRef second) {
    if (first == second) {
        return YES;
    }
    return first != NULL && second != NULL && CFEqual(first, second);
}

static GFTime GFTimeFromCMTime(CMTime time) {
    if (!CMTIME_IS_NUMERIC(time) || time.timescale <= 0) {
        return (GFTime){.numerator = 0, .denominator = 0};
    }
    return (GFTime){
        .numerator = time.value,
        .denominator = time.timescale,
    };
}

static NSString *GFRenderStateArchiveIdentity(GFRenderState *state) {
    GFRenderParameters parameters = state.parameters;
    NSData *parameterData = [NSData dataWithBytes:&parameters length:sizeof(parameters)];
    GFTimeRange effect = state.effectBounds;
    GFTimeRange input = state.inputBounds;
    return [NSString stringWithFormat:
        @"%ld|%@|%@|%@|%@|%lld/%lld/%lld/%lld|%lld/%lld/%lld/%lld|%ld",
        (long)state.schemaVersion,
        state.projectContentHash,
        state.projectDisplayName,
        state.timingPayload,
        [parameterData base64EncodedStringWithOptions:0],
        (long long)effect.start.numerator,
        (long long)effect.start.denominator,
        (long long)effect.duration.numerator,
        (long long)effect.duration.denominator,
        (long long)input.start.numerator,
        (long long)input.start.denominator,
        (long long)input.duration.numerator,
        (long long)input.duration.denominator,
        (long)state.mode];
}

@interface GFMetalDeviceResources : NSObject
@property(nonatomic, strong) id<MTLDevice> device;
@property(nonatomic, strong) id<MTLCommandQueue> commandQueue;
@end

@implementation GFMetalDeviceResources
@end

@interface GyroflowFinalCutEffect ()
@property(nonatomic, strong) GFProjectStore *projectStore;
@property(nonatomic, strong) GFRenderCache *renderCache;
@property(nonatomic, strong) GFParameterCommitter *parameterCommitter;
@property(nonatomic, strong) GFRenderDiagnostics *renderDiagnostics;
@property(nonatomic, strong) NSCache<NSNumber *, GFMetalDeviceResources *> *metalResources;
@property(nonatomic, strong) NSLock *pluginStateCacheLock;
@property(nonatomic, copy) NSString *cachedPluginStateIdentity;
@property(nonatomic, copy) NSData *cachedPluginStateData;
@property(nonatomic, weak) GFProjectDropView *projectView;
@property(nonatomic) BOOL renderStateRestoreScheduled;
@end

@implementation GyroflowFinalCutEffect

- (nullable instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager {
    self = [super init];
    if (self == nil) {
        return nil;
    }
    self.apiManager = apiManager;
    self.projectStore = [[GFProjectStore alloc] init];
    self.renderDiagnostics = [[GFRenderDiagnostics alloc]
        initWithClock:nil
               logger:^(NSString *message) {
                   os_log_info(OS_LOG_DEFAULT,
                               "Gyroflow Final Cut %{public}@",
                               message);
               }];
    self.renderCache = [[GFRenderCache alloc]
        initWithDiagnostics:self.renderDiagnostics];
    self.metalResources = [[NSCache alloc] init];
    self.metalResources.countLimit = 4;
    self.metalResources.totalCostLimit = 4;
    self.pluginStateCacheLock = [[NSLock alloc] init];
    self.parameterCommitter =
        [[GFParameterCommitter alloc] initWithAPIManager:apiManager];
    return self;
}

- (BOOL)addParametersWithError:(NSError **)error {
    id<FxParameterCreationAPI_v5> parameters =
        [self.apiManager apiForProtocol:@protocol(FxParameterCreationAPI_v5)];
    if (parameters == nil) {
        if (error != NULL) {
            *error = GFFxError(GFLocalized(
                @"effect.error.parameter_api",
                @"Final Cut parameter creation is unavailable"));
        }
        return NO;
    }
    BOOL ok = [parameters startParameterSubGroup:GFLocalized(
                                                       @"effect.group.project",
                                                       @"Gyroflow(Niyien) project")
                                     parameterID:kGFProjectGroup
                                  parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addCustomParameterWithName:GFLocalized(
                                                        @"effect.param.project_import",
                                                        @"Project Import")
                                            parameterID:kGFProjectControl
                                           defaultValue:@0
                                         parameterFlags:(kFxParameterFlag_CUSTOM_UI |
                                                         kFxParameterFlag_NOT_ANIMATABLE |
                                                         kFxParameterFlag_USE_FULL_VIEW_WIDTH)];
    ok = ok && [parameters endParameterSubGroup];
    ok = ok && [parameters addFloatSliderWithName:GFLocalized(@"effect.param.fov", @"FOV")
                                       parameterID:kGFFOV
                                      defaultValue:1.0
                                      parameterMin:0.1
                                      parameterMax:3.0
                                         sliderMin:0.1
                                         sliderMax:3.0
                                             delta:0.01
                                    parameterFlags:(kFxParameterFlag_HIDDEN |
                                                    kFxParameterFlag_DONT_DISPLAY_IN_DASHBOARD)];
    ok = ok && [parameters startParameterSubGroup:GFLocalized(
                                                       @"effect.group.adjust",
                                                       @"Adjust parameters")
                                       parameterID:kGFAdjustmentGroup
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:GFLocalized(
                                                     @"effect.param.smoothness",
                                                     @"Smoothness")
                                       parameterID:kGFSmoothness
                                      defaultValue:15.0
                                      parameterMin:1.0
                                      parameterMax:300.0
                                         sliderMin:1.0
                                         sliderMax:300.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:GFLocalized(
                                                     @"effect.param.lens_correction",
                                                     @"Lens correction")
                                       parameterID:kGFLensCorrection
                                      defaultValue:100.0
                                      parameterMin:0.0
                                      parameterMax:100.0
                                         sliderMin:0.0
                                         sliderMax:100.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:GFLocalized(
                                                     @"effect.param.horizon_lock",
                                                     @"Horizon lock")
                                       parameterID:kGFHorizonLockAmount
                                      defaultValue:0.0
                                      parameterMin:0.0
                                      parameterMax:100.0
                                         sliderMin:0.0
                                         sliderMax:100.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:GFLocalized(
                                                     @"effect.param.horizon_roll",
                                                     @"Horizon roll")
                                       parameterID:kGFHorizonLockRoll
                                      defaultValue:0.0
                                      parameterMin:-100.0
                                      parameterMax:100.0
                                         sliderMin:-100.0
                                         sliderMax:100.0
                                             delta:0.1
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addPopupMenuWithName:GFLocalized(
                                                    @"effect.param.zoom_mode",
                                                    @"Zoom mode")
                                     parameterID:kGFZoomMode
                                    defaultValue:1
                                     menuEntries:@[
                                         GFLocalized(@"effect.zoom.none", @"No zoom"),
                                         GFLocalized(@"effect.zoom.dynamic", @"Dynamic zoom"),
                                         GFLocalized(@"effect.zoom.static", @"Static zoom")
                                     ]
                                  parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addToggleButtonWithName:GFLocalized(
                                                      @"effect.param.overview",
                                                      @"Stabilization overview")
                                        parameterID:kGFOverview
                                       defaultValue:NO
                                     parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters endParameterSubGroup];
    ok = ok && [parameters addStringParameterWithName:@"Instance Identity"
                                            parameterID:kGFInstanceIdentity
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Project Payload"
                                            parameterID:kGFProjectPayload
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Timing Payload"
                                            parameterID:kGFTimingPayload
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Project Payload Manifest A"
                                            parameterID:kGFProjectPayloadManifestA
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Project Payload Manifest B"
                                            parameterID:kGFProjectPayloadManifestB
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Project Display Name"
                                            parameterID:kGFProjectDisplayName
                                           defaultValue:@""
                                         parameterFlags:(kFxParameterFlag_HIDDEN |
                                                         kFxParameterFlag_NOT_ANIMATABLE)];
    for (NSUInteger index = 0;
         index < kGFProjectPayloadChunksPerBank && ok;
         ++index) {
        NSString *name = [NSString stringWithFormat:@"Project Payload A %02lu",
                                                    (unsigned long)(index + 1)];
        ok = [parameters addStringParameterWithName:name
                                        parameterID:kGFProjectPayloadChunksA[index]
                                       defaultValue:@""
                                     parameterFlags:(kFxParameterFlag_HIDDEN |
                                                     kFxParameterFlag_NOT_ANIMATABLE)];
    }
    for (NSUInteger index = 0;
         index < kGFProjectPayloadChunksPerBank && ok;
         ++index) {
        NSString *name = [NSString stringWithFormat:@"Project Payload B %02lu",
                                                    (unsigned long)(index + 1)];
        ok = [parameters addStringParameterWithName:name
                                        parameterID:kGFProjectPayloadChunksB[index]
                                       defaultValue:@""
                                     parameterFlags:(kFxParameterFlag_HIDDEN |
                                                     kFxParameterFlag_NOT_ANIMATABLE)];
    }
    if (!ok && error != NULL) {
        *error = GFFxError(GFLocalized(
            @"effect.error.parameters_create",
            @"Final Cut failed to create Gyroflow parameters"));
    }
    return ok;
}

- (id<FxParameterRetrievalAPI_v6>)parameterRetrievalAPI {
    return [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
}

- (NSString *)stringValueForParameter:(UInt32)parameterID {
    NSString *value = nil;
    id<FxParameterRetrievalAPI_v6> retrieval = [self parameterRetrievalAPI];
    if (retrieval == nil ||
        ![retrieval getStringParameterValue:&value fromParameter:parameterID]) {
        return @"";
    }
    return value ?: @"";
}

- (void)restoreProjectStoreFromHostParameters {
    NSError *payloadError = nil;
    NSString *projectPayload =
        [self.parameterCommitter persistedProjectPayloadWithError:&payloadError];
    if (projectPayload == nil) {
        os_log_error(OS_LOG_DEFAULT,
                     "Gyroflow project restore rejected: %{public}@",
                     payloadError.localizedDescription);
        return;
    }
    if (projectPayload.length == 0) {
        return;
    }
    NSString *timingPayload = [self stringValueForParameter:kGFTimingPayload];
    NSString *displayName = [self stringValueForParameter:kGFProjectDisplayName];
    [self.projectStore restorePersistedProjectPayload:projectPayload
                                          displayName:displayName
                                        timingPayload:timingPayload
                                                error:NULL];
}

- (void)pluginInstanceAddedToDocument {
    // FxParameterRetrievalAPI is not available during this lifecycle callback.
    // Host-backed restoration happens when Final Cut asks for plugin state or
    // creates the custom parameter view, where the retrieval API is valid.
}

- (NSView *)createViewForParameterID:(UInt32)parameterID {
    if (parameterID != kGFProjectControl) {
        return nil;
    }
    __weak GyroflowFinalCutEffect *weakSelf = self;
    GFProjectDropView *view = [[GFProjectDropView alloc]
        initWithProjectStore:self.projectStore
               commitHandler:^BOOL(GFProjectImportCandidate *candidate, NSView *sender) {
                   GyroflowFinalCutEffect *strongSelf = weakSelf;
                   BOOL committed = candidate != nil &&
                       [strongSelf.parameterCommitter
                           commitProjectPayload:candidate.projectPayload
                                    displayName:candidate.projectDisplayName
                                     parameters:candidate.parameters
                                         sender:sender];
                   return committed;
               }];
    self.projectView = view;
    [self restoreProjectStoreFromHostParameters];
    [view refreshStatus];
    return view;
}

- (NSSet<Class> *)classesForCustomParameterID:(UInt32)parameterID {
    return parameterID == kGFProjectControl
        ? [NSSet setWithObject:[NSNumber class]]
        : [NSSet set];
}

- (void)restoreProjectStoreFromRenderSnapshotIfNeeded:(GFRenderSnapshot *)snapshot {
    if (snapshot.state.projectPayload.length == 0 ||
        (snapshot.preparationStatus != GF_STATUS_OK &&
         snapshot.preparationStatus != GF_STATUS_MISSING_TIMING)) {
        return;
    }
    @synchronized(self) {
        if (self.renderStateRestoreScheduled) {
            return;
        }
        self.renderStateRestoreScheduled = YES;
    }
    NSString *projectPayload = [snapshot.state.projectPayload copy];
    NSString *timingPayload = [snapshot.state.timingPayload copy];
    NSString *displayName = [snapshot.state.projectDisplayName copy];
    GFStatus preparationStatus = snapshot.preparationStatus;
    __weak GyroflowFinalCutEffect *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
        GyroflowFinalCutEffect *strongSelf = weakSelf;
        if (strongSelf == nil) {
            return;
        }
        if ([strongSelf.projectStore
                restoreValidatedRenderProjectPayloadIfEmpty:projectPayload
                                                displayName:displayName
                                              timingPayload:timingPayload]) {
            if (preparationStatus == GF_STATUS_MISSING_TIMING) {
                [strongSelf.projectStore recordReprocessRequired];
            }
            [strongSelf.projectView refreshStatus];
        }
    });
}

- (BOOL)properties:(NSDictionary * _Nonnull *)properties error:(NSError **)error {
    *properties = @{
        kFxPropertyKey_NeedsFullBuffer : @YES,
        kFxPropertyKey_VariesWhenParamsAreStatic : @YES,
        kFxPropertyKey_ChangesOutputSize : @NO,
        kFxPropertyKey_DesiredProcessingColorInfo : @(kFxImageColorInfo_RGB_LINEAR),
        kFxPropertyKey_PixelTransformSupport : @(kFxPixelTransform_ScaleTranslate),
    };
    return YES;
}

- (BOOL)pluginState:(NSData * _Nonnull *)pluginState
             atTime:(CMTime)renderTime
            quality:(FxQuality)qualityLevel
              error:(NSError **)error {
    NSTimeInterval pluginStateStarted = [NSDate timeIntervalSinceReferenceDate];
    id<FxParameterRetrievalAPI_v6> retrieval = [self parameterRetrievalAPI];
    if (retrieval == nil) {
        [self.projectStore failPendingHostReadbackWithMessage:GFLocalized(
            @"effect.error.parameter_retrieval",
            @"Final Cut parameter retrieval is unavailable")];
        if (error != NULL) {
            *error = GFFxError(GFLocalized(
                @"effect.error.parameter_retrieval",
                @"Final Cut parameter retrieval is unavailable"));
        }
        return NO;
    }
    NSUInteger projectReadbackGeneration =
        [self.projectStore beginHostProjectReadback];
    NSError *payloadError = nil;
    NSString *projectContentHash = nil;
    NSString *projectPayload =
        [self.parameterCommitter persistedProjectPayloadWithHash:&projectContentHash
                                                           error:&payloadError];
    if (projectPayload == nil) {
        NSString *detail = payloadError.localizedDescription
            ?: GFLocalized(@"effect.error.project_payload_invalid",
                           @"Project payload is invalid");
        [self.projectStore failPendingHostReadbackWithMessage:detail];
        if (error != NULL) {
            *error = GFFxError([NSString stringWithFormat:GFLocalized(
                @"effect.error.project_payload_detail",
                @"The project payload could not be read: %@"), detail]);
        }
        return NO;
    }
    NSString *timingPayload = [self stringValueForParameter:kGFTimingPayload];
    if ([self.projectStore
            reconcileHostPersistedProjectPayload:projectPayload
                              readbackGeneration:projectReadbackGeneration]) {
        if (timingPayload.length == 0) {
            [self.projectStore recordDirectModeReady];
        } else {
            [self.projectStore recordRouteDModeReady];
        }
        __weak GyroflowFinalCutEffect *weakSelf = self;
        dispatch_async(dispatch_get_main_queue(), ^{
            [weakSelf.projectView refreshStatus];
        });
    }
    NSString *displayName = [self stringValueForParameter:kGFProjectDisplayName];
    double fov = 1.0;
    double smoothness = 15.0;
    double lensCorrection = 100.0;
    double horizonLockAmount = 0.0;
    double horizonLockRoll = 0.0;
    int zoomMode = 1;
    BOOL overview = NO;
    BOOL parametersRead =
        [retrieval getFloatValue:&fov fromParameter:kGFFOV atTime:renderTime] &&
        [retrieval getFloatValue:&smoothness
                   fromParameter:kGFSmoothness
                          atTime:renderTime] &&
        [retrieval getFloatValue:&lensCorrection
                   fromParameter:kGFLensCorrection
                          atTime:renderTime] &&
        [retrieval getFloatValue:&horizonLockAmount
                   fromParameter:kGFHorizonLockAmount
                          atTime:renderTime] &&
        [retrieval getFloatValue:&horizonLockRoll
                   fromParameter:kGFHorizonLockRoll
                          atTime:renderTime] &&
        [retrieval getIntValue:&zoomMode
                 fromParameter:kGFZoomMode
                        atTime:renderTime] &&
        [retrieval getBoolValue:&overview
                  fromParameter:kGFOverview
                         atTime:renderTime];
    if (!parametersRead) {
        if (error != NULL) {
            *error = GFFxError(GFLocalized(
                @"effect.error.parameters_read",
                @"Final Cut did not provide all Gyroflow parameters"));
        }
        return NO;
    }
    GFRenderParameters parameters = {
        .fov = fov,
        .smoothness = smoothness,
        .lens_correction = lensCorrection,
        .horizon_lock_amount = horizonLockAmount,
        .horizon_lock_roll = horizonLockRoll,
        .zoom_mode = zoomMode,
        .overview = overview ? 1 : 0,
        .reserved = {0, 0, 0},
    };
    CMTime effectStart = kCMTimeInvalid;
    CMTime effectDuration = kCMTimeInvalid;
    CMTime inputStart = kCMTimeInvalid;
    CMTime inputDuration = kCMTimeInvalid;
    id<FxTimingAPI_v4> timing =
        [self.apiManager apiForProtocol:@protocol(FxTimingAPI_v4)];
    if (timing != nil) {
        [timing startTimeForEffect:&effectStart];
        [timing durationTimeForEffect:&effectDuration];
        [timing startTimeOfInputToFilter:&inputStart];
        [timing durationTimeOfInputToFilter:&inputDuration];
    }
    GFTimeRange effectBounds = {
        .start = GFTimeFromCMTime(effectStart),
        .duration = GFTimeFromCMTime(effectDuration),
    };
    GFTimeRange inputBounds = {
        .start = GFTimeFromCMTime(inputStart),
        .duration = GFTimeFromCMTime(inputDuration),
    };
    GFRenderMode mode = projectPayload.length == 0
        ? GFRenderModeEmpty
        : (timingPayload.length == 0 ? GFRenderModeDirect : GFRenderModeRouteD);
    GFRenderState *state = [[GFRenderState alloc]
        initWithProjectPayload:projectPayload
           projectDisplayName:displayName
           projectContentHash:projectContentHash ?: @""
               timingPayload:timingPayload
                        mode:mode
                  parameters:parameters
                effectBounds:effectBounds
                 inputBounds:inputBounds];
    NSString *archiveIdentity = GFRenderStateArchiveIdentity(state);
    [self.pluginStateCacheLock lock];
    NSData *archived = [self.cachedPluginStateIdentity isEqualToString:archiveIdentity]
        ? self.cachedPluginStateData
        : nil;
    [self.pluginStateCacheLock unlock];
    NSError *archiveError = nil;
    if (archived == nil) {
        archived = [NSKeyedArchiver archivedDataWithRootObject:state
                                         requiringSecureCoding:YES
                                                         error:&archiveError];
        if (archived != nil) {
            [self.pluginStateCacheLock lock];
            self.cachedPluginStateIdentity = archiveIdentity;
            self.cachedPluginStateData = archived;
            [self.pluginStateCacheLock unlock];
        }
    }
    if (archived == nil) {
        if (error != NULL) {
            NSString *detail = archiveError.localizedDescription
                ?: GFLocalized(@"effect.error.render_state_freeze",
                               @"Unable to freeze Gyroflow render state");
            *error = GFFxError([NSString stringWithFormat:GFLocalized(
                @"effect.error.render_state_freeze_detail",
                @"Gyroflow render state could not be saved: %@"), detail]);
        }
        return NO;
    }
    [self.renderCache snapshotForPluginStateData:archived error:NULL];
    [self.renderDiagnostics
        recordPluginStateBytes:archived.length
                 encodeSeconds:[NSDate timeIntervalSinceReferenceDate] - pluginStateStarted];
    *pluginState = archived;
    return YES;
}

- (BOOL)destinationImageRect:(FxRect *)destinationImageRect
                sourceImages:(NSArray<FxImageTile *> *)sourceImages
            destinationImage:(FxImageTile *)destinationImage
                 pluginState:(nullable NSData *)pluginState
                      atTime:(CMTime)renderTime
                       error:(NSError **)outError {
    if (sourceImages.count != 1) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.source_image_count",
                @"Gyroflow requires exactly one source image"));
        }
        return NO;
    }
    *destinationImageRect = destinationImage.imagePixelBounds;
    return YES;
}

- (BOOL)sourceTileRect:(FxRect *)sourceTileRect
      sourceImageIndex:(NSUInteger)sourceImageIndex
          sourceImages:(NSArray<FxImageTile *> *)sourceImages
   destinationTileRect:(FxRect)destinationTileRect
      destinationImage:(FxImageTile *)destinationImage
           pluginState:(nullable NSData *)pluginState
                atTime:(CMTime)renderTime
                 error:(NSError **)outError {
    if (sourceImageIndex >= sourceImages.count) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.source_image_unknown",
                @"Final Cut requested an unknown Gyroflow source image"));
        }
        return NO;
    }
    *sourceTileRect = destinationTileRect;
    return YES;
}

- (nullable GFMetalDeviceResources *)metalResourcesForRegistryID:(uint64_t)registryID {
    NSNumber *key = @(registryID);
    @synchronized(self.metalResources) {
        GFMetalDeviceResources *cached = [self.metalResources objectForKey:key];
        if (cached != nil) {
            return cached;
        }
        [self.renderDiagnostics recordDeviceEnumeration];
        for (id<MTLDevice> device in MTLCopyAllDevices()) {
            if (device.registryID != registryID) {
                continue;
            }
            id<MTLCommandQueue> queue = [device newCommandQueue];
            if (queue == nil) {
                return nil;
            }
            [self.renderDiagnostics recordCommandQueueCreation];
            GFMetalDeviceResources *resources = [[GFMetalDeviceResources alloc] init];
            resources.device = device;
            resources.commandQueue = queue;
            [self.metalResources setObject:resources forKey:key cost:1];
            return resources;
        }
    }
    return nil;
}

- (BOOL)copyTexture:(id<MTLTexture>)sourceTexture
          toTexture:(id<MTLTexture>)destinationTexture
       commandQueue:(id<MTLCommandQueue>)queue
              error:(NSError **)outError {
    if (sourceTexture.width != destinationTexture.width ||
        sourceTexture.height != destinationTexture.height ||
        sourceTexture.pixelFormat != destinationTexture.pixelFormat) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.passthrough_texture_mismatch",
                @"Safe passthrough requires matching source and destination textures"));
        }
        return NO;
    }
    id<MTLCommandBuffer> commandBuffer = [queue commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commandBuffer blitCommandEncoder];
    if (queue == nil || commandBuffer == nil || blit == nil) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.passthrough_command",
                @"Unable to create the Metal safe-passthrough command"));
        }
        return NO;
    }
    [blit copyFromTexture:sourceTexture
              sourceSlice:0
              sourceLevel:0
             sourceOrigin:MTLOriginMake(0, 0, 0)
               sourceSize:MTLSizeMake(sourceTexture.width, sourceTexture.height, 1)
                toTexture:destinationTexture
         destinationSlice:0
         destinationLevel:0
        destinationOrigin:MTLOriginMake(0, 0, 0)];
    [blit endEncoding];
    [commandBuffer commit];
    [commandBuffer waitUntilCompleted];
    if (commandBuffer.GPUEndTime >= commandBuffer.GPUStartTime &&
        commandBuffer.GPUStartTime > 0.0) {
        [self.renderDiagnostics
            recordGPUSeconds:commandBuffer.GPUEndTime - commandBuffer.GPUStartTime];
    }
    if (commandBuffer.status == MTLCommandBufferStatusError) {
        if (outError != NULL) {
            NSString *detail = commandBuffer.error.localizedDescription
                ?: GFLocalized(@"effect.error.passthrough_failed",
                               @"Metal safe passthrough failed");
            *outError = GFFxError([NSString stringWithFormat:GFLocalized(
                @"effect.error.passthrough_failed_detail",
                @"Metal safe passthrough failed: %@"), detail]);
        }
        return NO;
    }
    return YES;
}

- (BOOL)renderDestinationImage:(FxImageTile *)destinationImage
                  sourceImages:(NSArray<FxImageTile *> *)sourceImages
                   pluginState:(nullable NSData *)pluginState
                        atTime:(CMTime)renderTime
                         error:(NSError **)outError {
    NSTimeInterval frameStarted = [NSDate timeIntervalSinceReferenceDate];
    if (sourceImages.count != 1) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.render_source_count",
                @"Gyroflow received an unexpected source count"));
        }
        return NO;
    }
    NSError *stateError = nil;
    GFRenderSnapshot *snapshot =
        [self.renderCache snapshotForPluginStateData:pluginState ?: [NSData data]
                                              error:&stateError];
    if (snapshot == nil) {
        if (outError != NULL) {
            NSString *detail = stateError.localizedDescription
                ?: GFLocalized(@"effect.error.render_state_unavailable",
                               @"Immutable Gyroflow render state is unavailable");
            *outError = GFFxError([NSString stringWithFormat:GFLocalized(
                @"effect.error.render_state_detail",
                @"Gyroflow render state is unavailable: %@"), detail]);
        }
        return NO;
    }
    [self restoreProjectStoreFromRenderSnapshotIfNeeded:snapshot];
    FxImageTile *sourceImage = sourceImages.firstObject;
    if (sourceImage.deviceRegistryID == 0 ||
        sourceImage.deviceRegistryID != destinationImage.deviceRegistryID) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.device_registry",
                @"Final Cut supplied incompatible Metal device registry IDs"));
        }
        return NO;
    }
    GFMetalDeviceResources *metalResources =
        [self metalResourcesForRegistryID:destinationImage.deviceRegistryID];
    if (metalResources == nil) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.device_missing",
                @"No Metal device matches the Final Cut tile registry ID"));
        }
        return NO;
    }
    id<MTLDevice> device = metalResources.device;
    id<MTLTexture> sourceTexture = [sourceImage metalTextureForDevice:device];
    id<MTLTexture> destinationTexture = [destinationImage metalTextureForDevice:device];
    if (sourceTexture == nil || destinationTexture == nil) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.metal_textures",
                @"Unable to obtain IOSurface-backed Metal textures"));
        }
        return NO;
    }
    GFGeometryProbeRecordFrame(
        sourceImage,
        destinationImage,
        sourceTexture,
        destinationTexture,
        renderTime
    );
    if (sourceTexture.width != destinationTexture.width ||
        sourceTexture.height != destinationTexture.height ||
        sourceTexture.pixelFormat != destinationTexture.pixelFormat ||
        !GFColorSpacesEqual(sourceImage.colorSpace, destinationImage.colorSpace)) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.tile_semantics",
                @"Final Cut source and destination tile semantics do not match"));
        }
        return NO;
    }
    if (!CMTIME_IS_NUMERIC(renderTime) || renderTime.timescale <= 0 ||
        sourceTexture.width > (UINT32_MAX / 8) || sourceTexture.height > UINT32_MAX) {
        if (outError != NULL) {
            *outError = GFFxError(GFLocalized(
                @"effect.error.render_input_invalid",
                @"Final Cut supplied an invalid effect-local render time or frame size"));
        }
        return NO;
    }

    GFFrameGeometry frameGeometry = {0};
    GFError *bridgeError = NULL;
    GFStatus status = snapshot.preparationStatus;
    NSString *geometryErrorMessage = nil;
    [snapshot.renderLock lock];
    if (status == GF_STATUS_OK) {
        GFRenderParameters parameters = snapshot.state.parameters;
        status = gf_finalcut_instance_set_render_parameters(
            snapshot.instance,
            &parameters,
            &bridgeError
        );
    }
    if (status == GF_STATUS_OK) {
        GFProjectGeometry projectGeometry = {0};
        status = gf_finalcut_instance_get_project_geometry(
            snapshot.instance,
            &projectGeometry,
            &bridgeError
        );
        if (status == GF_STATUS_OK) {
            GFFrameGeometryImageSnapshot sourceGeometry = {
                .image_pixel_bounds = sourceImage.imagePixelBounds,
                .tile_pixel_bounds = sourceImage.tilePixelBounds,
                .pixel_transform = sourceImage.pixelTransform,
                .inverse_pixel_transform = sourceImage.inversePixelTransform,
                .image_origin = sourceImage.imageOrigin,
                .texture_dimensions = {
                    .width = (uint32_t)sourceTexture.width,
                    .height = (uint32_t)sourceTexture.height,
                },
            };
            GFFrameGeometryImageSnapshot destinationGeometry = {
                .image_pixel_bounds = destinationImage.imagePixelBounds,
                .tile_pixel_bounds = destinationImage.tilePixelBounds,
                .pixel_transform = destinationImage.pixelTransform,
                .inverse_pixel_transform = destinationImage.inversePixelTransform,
                .image_origin = destinationImage.imageOrigin,
                .texture_dimensions = {
                    .width = (uint32_t)destinationTexture.width,
                    .height = (uint32_t)destinationTexture.height,
                },
            };
            NSError *geometryError = nil;
            if (!GFFrameGeometryBuild(
                    sourceGeometry,
                    destinationGeometry,
                    projectGeometry,
                    &frameGeometry,
                    &geometryError)) {
                status = GF_STATUS_INVALID_ARGUMENT;
                geometryErrorMessage = geometryError.localizedDescription
                    ?: GFLocalized(@"effect.error.frame_geometry",
                                   @"Final Cut frame geometry is unsupported");
            }
        }
    }
    GFMetalRenderRequest request = {
        .input_texture = (__bridge void *)sourceTexture,
        .output_texture = (__bridge void *)destinationTexture,
        .command_queue = (__bridge void *)metalResources.commandQueue,
        .device_registry_id = destinationImage.deviceRegistryID,
        .width = (uint32_t)sourceTexture.width,
        .height = (uint32_t)sourceTexture.height,
        .input_row_bytes = (uint32_t)sourceTexture.width * 8,
        .output_row_bytes = (uint32_t)destinationTexture.width * 8,
        .pixel_format = (uint32_t)sourceTexture.pixelFormat,
        .effect_local_time = {
            .numerator = renderTime.value,
            .denominator = renderTime.timescale,
        },
        .effect_bounds = snapshot.state.effectBounds,
        .input_bounds = snapshot.state.inputBounds,
        .geometry = frameGeometry,
    };
    if (status == GF_STATUS_OK) {
        status = gf_finalcut_instance_render_metal(
            snapshot.instance,
            &request,
            &bridgeError
        );
    }
    GFRenderDisposition disposition = GFRenderDispositionForStatus(status);
    if (disposition == GF_RENDER_DISPOSITION_PROCESSED) {
        [snapshot.renderLock unlock];
        [self.renderDiagnostics
            recordFrameSeconds:[NSDate timeIntervalSinceReferenceDate] - frameStarted
                     processed:YES];
        gf_finalcut_error_free(bridgeError);
        return YES;
    }
    NSString *message = bridgeError != NULL
        ? GFBridgeErrorMessage(
              bridgeError,
              GFLocalized(@"effect.error.render_failed", @"Gyroflow Metal render failed"))
        : geometryErrorMessage ?: snapshot.statusMessage;
    if (disposition == GF_RENDER_DISPOSITION_PASSTHROUGH) {
        [snapshot.renderLock unlock];
        [self.renderDiagnostics recordPassthroughStatus:status reason:message ?: @""];
        gf_finalcut_error_free(bridgeError);
        BOOL copied = [self copyTexture:sourceTexture
                              toTexture:destinationTexture
                           commandQueue:metalResources.commandQueue
                                  error:outError];
        [self.renderDiagnostics
            recordFrameSeconds:[NSDate timeIntervalSinceReferenceDate] - frameStarted
                     processed:NO];
        return copied;
    }
    [snapshot.renderLock unlock];
    if (outError != NULL) {
        *outError = GFFxError([NSString stringWithFormat:GFLocalized(
            @"effect.error.render_detail", @"Gyroflow could not render this frame: %@"),
            message ?: GFLocalized(@"effect.error.render_failed",
                                   @"Gyroflow Metal render failed")]);
    }
    gf_finalcut_error_free(bridgeError);
    return NO;
}

@end
