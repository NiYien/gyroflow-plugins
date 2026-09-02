#import "GFPhase0Effect.h"

#import "GFPhase0DropZoneView.h"
#import "GFPhase0ParameterCommitter.h"
#import "GFPhase0ProjectStore.h"
#import <Metal/Metal.h>
#import <os/log.h>
#import <stdatomic.h>


#ifndef GF_PHASE0_PROCESSING_COLOR_INFO
#define GF_PHASE0_PROCESSING_COLOR_INFO -1
#endif


static const UInt32 kGFPhase0Group = 1100;
static const UInt32 kGFPhase0DropZone = 1101;
static const UInt32 kGFPhase0ObservedRenderTime = 1102;
static const UInt32 kGFPhase0LastDropEvidence = 1103;
static const UInt32 kGFPhase0InstanceIdentity = 1901;
static const UInt32 kGFPhase0ProjectPayload = 1902;
static const UInt32 kGFPhase0TimingPayload = 1903;

static _Atomic(double) gLastRenderTimestampSeconds = NAN;


double GFPhase0LastRenderTimestampSeconds(void) {
    return atomic_load_explicit(&gLastRenderTimestampSeconds, memory_order_relaxed);
}


static NSError *GFPhase0Error(NSString *message) {
    return [NSError errorWithDomain:FxPlugErrorDomain
                               code:(kFxError_ThirdPartyDeveloperStart + 1)
                           userInfo:@{NSLocalizedDescriptionKey : message}];
}


static NSString *GFPhase0ColorSpaceName(CGColorSpaceRef colorSpace) {
    if (colorSpace == nil) {
        return @"nil";
    }
    CFStringRef copiedName = CGColorSpaceCopyName(colorSpace);
    return CFBridgingRelease(copiedName) ?: @"unnamed";
}


static NSString *GFPhase0TimeDescription(CMTime time) {
    return [NSString stringWithFormat:@"%lld/%d flags=%u epoch=%lld seconds=%.9f",
                                      time.value,
                                      time.timescale,
                                      time.flags,
                                      time.epoch,
                                      CMTimeGetSeconds(time)];
}


@implementation GFPhase0Effect

- (nullable instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager {
    self = [super init];
    if (self != nil) {
        self.apiManager = apiManager;
        self.projectStore = [[GFPhase0ProjectStore alloc] init];
        os_log_info(OS_LOG_DEFAULT, "phase0 xpc initialized pid=%{public}d", getpid());
    }
    return self;
}

- (id<FxParameterRetrievalAPI_v6>)parameterRetrievalAPI {
    return [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
}

- (NSString *)stringValueForParameter:(UInt32)parameterID {
    id<FxParameterRetrievalAPI_v6> retrieval = [self parameterRetrievalAPI];
    NSString *value = nil;
    if (retrieval == nil || ![retrieval getStringParameterValue:&value fromParameter:parameterID]) {
        return @"";
    }
    return value ?: @"";
}

- (void)persistProjectData:(NSData *)projectData sender:(NSView *)sender {
    GFPhase0ParameterCommitter *committer =
        [[GFPhase0ParameterCommitter alloc] initWithAPIManager:self.apiManager
                                  instanceIdentityParameterID:kGFPhase0InstanceIdentity
                                     projectPayloadParameterID:kGFPhase0ProjectPayload
                                           evidenceParameterID:kGFPhase0LastDropEvidence];
    NSString *identity = nil;
    BOOL committed = [committer commitProjectData:projectData
                                           sender:sender
                                         identity:&identity];
    os_log_info(OS_LOG_DEFAULT,
                "phase0 project payload persisted committed=%{public}d instance=%{public}@ project_bytes=%{public}lu",
                committed,
                identity,
                (unsigned long)projectData.length);
}

- (BOOL)addParametersWithError:(NSError **)error {
    id<FxParameterCreationAPI_v5> parameters = [self.apiManager apiForProtocol:@protocol(FxParameterCreationAPI_v5)];
    if (parameters == nil) {
        if (error != NULL) {
            *error = GFPhase0Error(@"FxParameterCreationAPI_v5 is unavailable");
        }
        return NO;
    }

    BOOL ok = [parameters startParameterSubGroup:@"Phase 0 Probe"
                                     parameterID:kGFPhase0Group
                                  parameterFlags:kFxParameterFlag_DEFAULT];
    ok = ok && [parameters addCustomParameterWithName:@"Drop One Clip or File"
                                           parameterID:kGFPhase0DropZone
                                          defaultValue:@0
                                        parameterFlags:(kFxParameterFlag_CUSTOM_UI |
                                                        kFxParameterFlag_NOT_ANIMATABLE |
                                                        kFxParameterFlag_USE_FULL_VIEW_WIDTH)];
    ok = ok && [parameters addFloatSliderWithName:@"Observed Render Timestamp"
                                       parameterID:kGFPhase0ObservedRenderTime
                                      defaultValue:0.0
                                      parameterMin:-86400.0
                                      parameterMax:86400.0
                                         sliderMin:-10.0
                                         sliderMax:10.0
                                             delta:0.001
                                    parameterFlags:(kFxParameterFlag_DISABLED |
                                                    kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Last Drop Evidence"
                                           parameterID:kGFPhase0LastDropEvidence
                                          defaultValue:@"Waiting for one item"
                                        parameterFlags:(kFxParameterFlag_DISABLED |
                                                        kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters endParameterSubGroup];
    ok = ok && [parameters addStringParameterWithName:@"Instance Identity"
                                           parameterID:kGFPhase0InstanceIdentity
                                          defaultValue:@""
                                        parameterFlags:(kFxParameterFlag_HIDDEN |
                                                        kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Project Payload"
                                           parameterID:kGFPhase0ProjectPayload
                                          defaultValue:@""
                                        parameterFlags:(kFxParameterFlag_HIDDEN |
                                                        kFxParameterFlag_NOT_ANIMATABLE)];
    ok = ok && [parameters addStringParameterWithName:@"Timing Payload"
                                           parameterID:kGFPhase0TimingPayload
                                          defaultValue:@""
                                        parameterFlags:(kFxParameterFlag_HIDDEN |
                                                        kFxParameterFlag_NOT_ANIMATABLE)];
    if (!ok && error != NULL) {
        *error = GFPhase0Error(@"Failed to create Phase 0 parameters");
    }
    return ok;
}

- (NSView *)createViewForParameterID:(UInt32)parameterID {
    os_log_info(OS_LOG_DEFAULT,
                "phase0 custom view requested parameter_id=%{public}u",
                parameterID);
    if (parameterID != kGFPhase0DropZone) {
        return nil;
    }
    __weak GFPhase0Effect *weakSelf = self;
    return [[GFPhase0DropZoneView alloc]
        initWithProjectStore:self.projectStore
               commitHandler:^(NSData *projectData, NSView *sender) {
                   [weakSelf persistProjectData:projectData sender:sender];
               }];
}

- (NSSet<Class> *)classesForCustomParameterID:(UInt32)parameterID {
    if (parameterID == kGFPhase0DropZone) {
        return [NSSet setWithObject:[NSNumber class]];
    }
    return [NSSet set];
}

- (BOOL)properties:(NSDictionary * _Nonnull *)properties error:(NSError **)error {
    NSMutableDictionary *values = [@{
        kFxPropertyKey_NeedsFullBuffer : @YES,
        kFxPropertyKey_VariesWhenParamsAreStatic : @YES,
        kFxPropertyKey_ChangesOutputSize : @NO,
    } mutableCopy];
#if GF_PHASE0_PROCESSING_COLOR_INFO >= 0
    values[kFxPropertyKey_DesiredProcessingColorInfo] = @(GF_PHASE0_PROCESSING_COLOR_INFO);
#endif
    *properties = [values copy];
    return YES;
}

- (BOOL)pluginState:(NSData * _Nonnull *)pluginState
             atTime:(CMTime)renderTime
            quality:(FxQuality)qualityLevel
              error:(NSError **)error {
    NSString *identity = [self stringValueForParameter:kGFPhase0InstanceIdentity];
    NSString *projectPayload = [self stringValueForParameter:kGFPhase0ProjectPayload];
    NSString *timingPayload = [self stringValueForParameter:kGFPhase0TimingPayload];
    id<FxTimingAPI_v4> timingAPI =
        [self.apiManager apiForProtocol:@protocol(FxTimingAPI_v4)];
    CMTime inputTime = kCMTimeInvalid;
    CMTime roundTripTime = kCMTimeInvalid;
    CMTime effectStartTime = kCMTimeInvalid;
    CMTime inputStartTime = kCMTimeInvalid;
    CMTime timelineInTime = kCMTimeInvalid;
    CMTime effectLocalTime = kCMTimeInvalid;
    CMTime timelineProbeTime = kCMTimeInvalid;
    CMTime mappedInputTime = kCMTimeInvalid;
    CMTime mappedRoundTripTime = kCMTimeInvalid;
    if (timingAPI != nil) {
        [timingAPI startTimeForEffect:&effectStartTime];
        [timingAPI startTimeOfInputToFilter:&inputStartTime];
        [timingAPI inPointTimeOfTimelineForEffect:&timelineInTime];
        [timingAPI inputTime:&inputTime fromTimelineTime:renderTime];
        [timingAPI timelineTime:&roundTripTime fromInputTime:inputTime];
        effectLocalTime = CMTimeSubtract(renderTime, effectStartTime);
        timelineProbeTime = CMTimeAdd(timelineInTime, effectLocalTime);
        [timingAPI inputTime:&mappedInputTime fromTimelineTime:timelineProbeTime];
        [timingAPI timelineTime:&mappedRoundTripTime fromInputTime:mappedInputTime];
    }
    NSDictionary *state = @{
        @"renderTimeValue" : @(renderTime.value),
        @"renderTimeScale" : @(renderTime.timescale),
        @"inputTimeValue" : @(inputTime.value),
        @"inputTimeScale" : @(inputTime.timescale),
        @"roundTripTimeValue" : @(roundTripTime.value),
        @"roundTripTimeScale" : @(roundTripTime.timescale),
        @"effectStartTimeValue" : @(effectStartTime.value),
        @"effectStartTimeScale" : @(effectStartTime.timescale),
        @"inputStartTimeValue" : @(inputStartTime.value),
        @"inputStartTimeScale" : @(inputStartTime.timescale),
        @"timelineInTimeValue" : @(timelineInTime.value),
        @"timelineInTimeScale" : @(timelineInTime.timescale),
        @"effectLocalTimeValue" : @(effectLocalTime.value),
        @"effectLocalTimeScale" : @(effectLocalTime.timescale),
        @"timelineProbeTimeValue" : @(timelineProbeTime.value),
        @"timelineProbeTimeScale" : @(timelineProbeTime.timescale),
        @"mappedInputTimeValue" : @(mappedInputTime.value),
        @"mappedInputTimeScale" : @(mappedInputTime.timescale),
        @"mappedRoundTripTimeValue" : @(mappedRoundTripTime.value),
        @"mappedRoundTripTimeScale" : @(mappedRoundTripTime.timescale),
        @"instanceIdentity" : identity,
        @"projectPayload" : projectPayload,
        @"timingPayload" : timingPayload,
    };
    os_log_info(OS_LOG_DEFAULT,
                "phase0 timing probe available=%{public}d render=%{public}@ input=%{public}@ roundtrip=%{public}@ effect_start=%{public}@ input_start=%{public}@ timeline_in=%{public}@ effect_local=%{public}@ timeline_probe=%{public}@ mapped_input=%{public}@ mapped_roundtrip=%{public}@",
                timingAPI != nil,
                GFPhase0TimeDescription(renderTime),
                GFPhase0TimeDescription(inputTime),
                GFPhase0TimeDescription(roundTripTime),
                GFPhase0TimeDescription(effectStartTime),
                GFPhase0TimeDescription(inputStartTime),
                GFPhase0TimeDescription(timelineInTime),
                GFPhase0TimeDescription(effectLocalTime),
                GFPhase0TimeDescription(timelineProbeTime),
                GFPhase0TimeDescription(mappedInputTime),
                GFPhase0TimeDescription(mappedRoundTripTime));
    os_log_info(OS_LOG_DEFAULT,
                "phase0 plugin state instance=%{public}@ project_base64_bytes=%{public}lu timing_bytes=%{public}lu",
                identity,
                (unsigned long)projectPayload.length,
                (unsigned long)timingPayload.length);
    *pluginState = [NSJSONSerialization dataWithJSONObject:state options:0 error:error];
    return *pluginState != nil;
}

- (BOOL)destinationImageRect:(FxRect *)destinationImageRect
                sourceImages:(NSArray<FxImageTile *> *)sourceImages
            destinationImage:(FxImageTile *)destinationImage
                 pluginState:(nullable NSData *)pluginState
                      atTime:(CMTime)renderTime
                       error:(NSError **)outError {
    if (sourceImages.count != 1) {
        if (outError != NULL) {
            *outError = GFPhase0Error(@"Passthrough requires exactly one source image");
        }
        return NO;
    }
    *destinationImageRect = sourceImages.firstObject.imagePixelBounds;
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
    *sourceTileRect = destinationTileRect;
    return YES;
}

- (BOOL)renderDestinationImage:(FxImageTile *)destinationImage
                  sourceImages:(NSArray<FxImageTile *> *)sourceImages
                   pluginState:(nullable NSData *)pluginState
                        atTime:(CMTime)renderTime
                         error:(NSError **)outError {
    if (sourceImages.count != 1) {
        if (outError != NULL) {
            *outError = GFPhase0Error(@"Passthrough received an unexpected source count");
        }
        return NO;
    }

    FxImageTile *sourceImage = sourceImages.firstObject;
    id<MTLDevice> selectedDevice = nil;
    for (id<MTLDevice> device in MTLCopyAllDevices()) {
        if (device.registryID == destinationImage.deviceRegistryID) {
            selectedDevice = device;
            break;
        }
    }
    if (selectedDevice == nil) {
        selectedDevice = MTLCreateSystemDefaultDevice();
    }

    id<MTLTexture> sourceTexture = [sourceImage metalTextureForDevice:selectedDevice];
    id<MTLTexture> destinationTexture = [destinationImage metalTextureForDevice:selectedDevice];
    if (sourceTexture == nil || destinationTexture == nil) {
        if (outError != NULL) {
            *outError = GFPhase0Error(@"Unable to obtain Metal textures from FxImageTile");
        }
        return NO;
    }

    id<FxColorGamutAPI_v2> colorGamut =
        [self.apiManager apiForProtocol:@protocol(FxColorGamutAPI_v2)];
    NSInteger colorPrimaries = colorGamut != nil ? (NSInteger)colorGamut.colorPrimaries : -1;
    CGColorSpaceRef sourceColorSpace = sourceImage.colorSpace;
    CGColorSpaceRef destinationColorSpace = destinationImage.colorSpace;
    os_log_info(OS_LOG_DEFAULT,
                "phase0 color probe desired=%{public}d primaries=%{public}ld source_space=%{public}@ source_extended=%{public}d source_wide=%{public}d source_pixel_format=%{public}lu destination_space=%{public}@ destination_extended=%{public}d destination_wide=%{public}d destination_pixel_format=%{public}lu",
                GF_PHASE0_PROCESSING_COLOR_INFO,
                (long)colorPrimaries,
                GFPhase0ColorSpaceName(sourceColorSpace),
                sourceColorSpace != nil && CGColorSpaceUsesExtendedRange(sourceColorSpace),
                sourceColorSpace != nil && CGColorSpaceIsWideGamutRGB(sourceColorSpace),
                (unsigned long)sourceTexture.pixelFormat,
                GFPhase0ColorSpaceName(destinationColorSpace),
                destinationColorSpace != nil && CGColorSpaceUsesExtendedRange(destinationColorSpace),
                destinationColorSpace != nil && CGColorSpaceIsWideGamutRGB(destinationColorSpace),
                (unsigned long)destinationTexture.pixelFormat);

    id<MTLCommandQueue> queue = [selectedDevice newCommandQueue];
    id<MTLCommandBuffer> commandBuffer = [queue commandBuffer];
    id<MTLBlitCommandEncoder> blit = [commandBuffer blitCommandEncoder];
    MTLSize size = MTLSizeMake(MIN(sourceTexture.width, destinationTexture.width),
                               MIN(sourceTexture.height, destinationTexture.height),
                               1);
    [blit copyFromTexture:sourceTexture
              sourceSlice:0
              sourceLevel:0
             sourceOrigin:MTLOriginMake(0, 0, 0)
               sourceSize:size
                toTexture:destinationTexture
         destinationSlice:0
         destinationLevel:0
        destinationOrigin:MTLOriginMake(0, 0, 0)];
    [blit endEncoding];
    [commandBuffer commit];
    [commandBuffer waitUntilCompleted];
    if (commandBuffer.status == MTLCommandBufferStatusError) {
        if (outError != NULL) {
            *outError = commandBuffer.error ?: GFPhase0Error(@"Metal passthrough failed");
        }
        return NO;
    }

    double renderSeconds = CMTimeGetSeconds(renderTime);
    double sourceSeconds = CMTimeGetSeconds(sourceImage.mediaTime);
    atomic_store_explicit(&gLastRenderTimestampSeconds, renderSeconds, memory_order_relaxed);
    os_log_info(OS_LOG_DEFAULT,
                "phase0 render render_value=%{public}lld render_scale=%{public}d render_seconds=%{public}.9f source_value=%{public}lld source_scale=%{public}d source_seconds=%{public}.9f registry_id=%{public}llu",
                renderTime.value,
                renderTime.timescale,
                renderSeconds,
                sourceImage.mediaTime.value,
                sourceImage.mediaTime.timescale,
                sourceSeconds,
                destinationImage.deviceRegistryID);
    return YES;
}

@end
