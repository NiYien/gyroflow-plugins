#import "GyroflowFinalCutEffect.h"

#import "GFFrameGeometryAdapter.h"
#import "GFGeometryProbe.h"
#import "GFParameterCommitter.h"
#import "GFParameterIDs.h"
#import "GFProjectDropView.h"
#import "GFProjectStore.h"
#import "GFRenderCache.h"
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

@interface GyroflowFinalCutEffect ()
@property(nonatomic, strong) GFProjectStore *projectStore;
@property(nonatomic, strong) GFRenderCache *renderCache;
@property(nonatomic, strong) GFParameterCommitter *parameterCommitter;
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
    self.renderCache = [[GFRenderCache alloc] init];
    self.parameterCommitter =
        [[GFParameterCommitter alloc] initWithAPIManager:apiManager];
    return self;
}

- (BOOL)addParametersWithError:(NSError **)error {
    id<FxParameterCreationAPI_v5> parameters =
        [self.apiManager apiForProtocol:@protocol(FxParameterCreationAPI_v5)];
    if (parameters == nil) {
        if (error != NULL) {
            *error = GFFxError(@"FxParameterCreationAPI_v5 is unavailable");
        }
        return NO;
    }
    BOOL ok = [parameters startParameterSubGroup:@"Gyroflow Project"
                                     parameterID:kGFProjectGroup
                                  parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addCustomParameterWithName:@"Project Import"
                                            parameterID:kGFProjectControl
                                           defaultValue:@0
                                         parameterFlags:(kFxParameterFlag_CUSTOM_UI |
                                                         kFxParameterFlag_NOT_ANIMATABLE |
                                                         kFxParameterFlag_USE_FULL_VIEW_WIDTH)];
    ok = ok && [parameters endParameterSubGroup];
    ok = ok && [parameters startParameterSubGroup:@"Stabilization"
                                       parameterID:kGFAdjustmentGroup
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:@"FOV"
                                       parameterID:kGFFOV
                                      defaultValue:1.0
                                      parameterMin:0.1
                                      parameterMax:3.0
                                         sliderMin:0.1
                                         sliderMax:3.0
                                             delta:0.01
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:@"Smoothness"
                                       parameterID:kGFSmoothness
                                      defaultValue:50.0
                                      parameterMin:1.0
                                      parameterMax:300.0
                                         sliderMin:1.0
                                         sliderMax:300.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:@"Lens Correction"
                                       parameterID:kGFLensCorrection
                                      defaultValue:100.0
                                      parameterMin:0.0
                                      parameterMax:100.0
                                         sliderMin:0.0
                                         sliderMax:100.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:@"Horizon Lock"
                                       parameterID:kGFHorizonLockAmount
                                      defaultValue:0.0
                                      parameterMin:0.0
                                      parameterMax:100.0
                                         sliderMin:0.0
                                         sliderMax:100.0
                                             delta:1.0
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addFloatSliderWithName:@"Horizon Roll"
                                       parameterID:kGFHorizonLockRoll
                                      defaultValue:0.0
                                      parameterMin:-100.0
                                      parameterMax:100.0
                                         sliderMin:-100.0
                                         sliderMax:100.0
                                             delta:0.1
                                    parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addPopupMenuWithName:@"Zoom Mode"
                                     parameterID:kGFZoomMode
                                    defaultValue:1
                                     menuEntries:@[@"No zoom", @"Dynamic zoom", @"Static zoom"]
                                  parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addToggleButtonWithName:@"Overview"
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
        *error = GFFxError(@"Final Cut failed to create Gyroflow parameters");
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
    [self.projectStore restorePersistedProjectPayload:projectPayload
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
               commitHandler:^BOOL(NSString *payload, NSView *sender) {
                   BOOL committed =
                       [weakSelf.parameterCommitter commitProjectPayload:payload
                                                                  sender:sender];
                   if (committed &&
                       [[weakSelf stringValueForParameter:kGFTimingPayload] length] == 0) {
                       [weakSelf.projectStore recordDirectModeReady];
                   }
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
    if (snapshot.preparationStatus != GF_STATUS_OK ||
        snapshot.state.projectPayload.length == 0) {
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
    __weak GyroflowFinalCutEffect *weakSelf = self;
    dispatch_async(dispatch_get_main_queue(), ^{
        GyroflowFinalCutEffect *strongSelf = weakSelf;
        if (strongSelf == nil) {
            return;
        }
        if ([strongSelf.projectStore
                restoreValidatedRenderProjectPayloadIfEmpty:projectPayload
                                              timingPayload:timingPayload]) {
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
    id<FxParameterRetrievalAPI_v6> retrieval = [self parameterRetrievalAPI];
    if (retrieval == nil) {
        if (error != NULL) {
            *error = GFFxError(@"FxParameterRetrievalAPI_v6 is unavailable");
        }
        return NO;
    }
    NSError *payloadError = nil;
    NSString *projectPayload =
        [self.parameterCommitter persistedProjectPayloadWithError:&payloadError];
    if (projectPayload == nil) {
        if (error != NULL) {
            *error = payloadError ?: GFFxError(@"Project payload is invalid");
        }
        return NO;
    }
    if ([self.projectStore reconcileHostPersistedProjectPayload:projectPayload]) {
        __weak GyroflowFinalCutEffect *weakSelf = self;
        dispatch_async(dispatch_get_main_queue(), ^{
            [weakSelf.projectView refreshStatus];
        });
    }
    NSString *timingPayload = [self stringValueForParameter:kGFTimingPayload];
    double fov = 1.0;
    double smoothness = 50.0;
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
            *error = GFFxError(@"Final Cut did not provide all Gyroflow parameters");
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
    GFRenderState *state =
        [[GFRenderState alloc] initWithProjectPayload:projectPayload
                                       timingPayload:timingPayload
                                          parameters:parameters
                                        effectBounds:effectBounds
                                         inputBounds:inputBounds];
    NSError *archiveError = nil;
    NSData *archived = [NSKeyedArchiver archivedDataWithRootObject:state
                                            requiringSecureCoding:YES
                                                            error:&archiveError];
    if (archived == nil) {
        if (error != NULL) {
            *error = archiveError ?: GFFxError(@"Unable to freeze Gyroflow render state");
        }
        return NO;
    }
    [self.renderCache snapshotForPluginStateData:archived error:NULL];
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
            *outError = GFFxError(@"Gyroflow requires exactly one source image");
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
            *outError = GFFxError(@"Final Cut requested an unknown Gyroflow source image");
        }
        return NO;
    }
    *sourceTileRect = destinationTileRect;
    return YES;
}

- (nullable id<MTLDevice>)deviceForRegistryID:(uint64_t)registryID {
    for (id<MTLDevice> device in MTLCopyAllDevices()) {
        if (device.registryID == registryID) {
            return device;
        }
    }
    return nil;
}

- (BOOL)copyTexture:(id<MTLTexture>)sourceTexture
          toTexture:(id<MTLTexture>)destinationTexture
             device:(id<MTLDevice>)device
              error:(NSError **)outError {
    if (sourceTexture.width != destinationTexture.width ||
        sourceTexture.height != destinationTexture.height ||
        sourceTexture.pixelFormat != destinationTexture.pixelFormat) {
        if (outError != NULL) {
            *outError = GFFxError(@"Safe passthrough requires matching source and destination textures");
        }
        return NO;
    }
    id<MTLCommandQueue> queue = [device newCommandQueue];
    id<MTLCommandBuffer> commandBuffer = [queue commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commandBuffer blitCommandEncoder];
    if (queue == nil || commandBuffer == nil || blit == nil) {
        if (outError != NULL) {
            *outError = GFFxError(@"Unable to create the Metal safe-passthrough command");
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
    if (commandBuffer.status == MTLCommandBufferStatusError) {
        if (outError != NULL) {
            *outError = commandBuffer.error ?: GFFxError(@"Metal safe passthrough failed");
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
    if (sourceImages.count != 1) {
        if (outError != NULL) {
            *outError = GFFxError(@"Gyroflow received an unexpected source count");
        }
        return NO;
    }
    NSError *stateError = nil;
    GFRenderSnapshot *snapshot =
        [self.renderCache snapshotForPluginStateData:pluginState ?: [NSData data]
                                              error:&stateError];
    if (snapshot == nil) {
        if (outError != NULL) {
            *outError = stateError ?: GFFxError(@"Immutable Gyroflow render state is unavailable");
        }
        return NO;
    }
    [self restoreProjectStoreFromRenderSnapshotIfNeeded:snapshot];
    FxImageTile *sourceImage = sourceImages.firstObject;
    if (sourceImage.deviceRegistryID == 0 ||
        sourceImage.deviceRegistryID != destinationImage.deviceRegistryID) {
        if (outError != NULL) {
            *outError = GFFxError(@"Final Cut supplied incompatible Metal device registry IDs");
        }
        return NO;
    }
    id<MTLDevice> device = [self deviceForRegistryID:destinationImage.deviceRegistryID];
    if (device == nil) {
        if (outError != NULL) {
            *outError = GFFxError(@"No Metal device matches the Final Cut tile registry ID");
        }
        return NO;
    }
    id<MTLTexture> sourceTexture = [sourceImage metalTextureForDevice:device];
    id<MTLTexture> destinationTexture = [destinationImage metalTextureForDevice:device];
    if (sourceTexture == nil || destinationTexture == nil) {
        if (outError != NULL) {
            *outError = GFFxError(@"Unable to obtain IOSurface-backed Metal textures");
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
            *outError = GFFxError(@"Final Cut source and destination tile semantics do not match");
        }
        return NO;
    }
    if (!CMTIME_IS_NUMERIC(renderTime) || renderTime.timescale <= 0 ||
        sourceTexture.width > (UINT32_MAX / 8) || sourceTexture.height > UINT32_MAX) {
        if (outError != NULL) {
            *outError = GFFxError(@"Final Cut supplied an invalid effect-local render time or frame size");
        }
        return NO;
    }

    GFFrameGeometry frameGeometry = {0};
    GFError *bridgeError = NULL;
    GFStatus status = snapshot.preparationStatus;
    NSString *geometryErrorMessage = nil;
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
                    ?: @"Final Cut frame geometry is unsupported";
            }
        }
    }
    GFMetalRenderRequest request = {
        .input_texture = (__bridge void *)sourceTexture,
        .output_texture = (__bridge void *)destinationTexture,
        .command_queue = NULL,
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
        gf_finalcut_error_free(bridgeError);
        return YES;
    }
    NSString *message = bridgeError != NULL
        ? GFBridgeErrorMessage(bridgeError, @"Gyroflow Metal render failed")
        : geometryErrorMessage ?: snapshot.statusMessage;
    if (disposition == GF_RENDER_DISPOSITION_PASSTHROUGH) {
        os_log_info(OS_LOG_DEFAULT,
                    "Gyroflow Final Cut safe passthrough status=%{public}d reason=%{public}@",
                    status,
                    message);
        gf_finalcut_error_free(bridgeError);
        return [self copyTexture:sourceTexture
                       toTexture:destinationTexture
                          device:device
                           error:outError];
    }
    if (outError != NULL) {
        *outError = GFFxError(message);
    }
    gf_finalcut_error_free(bridgeError);
    return NO;
}

@end
