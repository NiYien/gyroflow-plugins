#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>
#import <objc/runtime.h>
#include <string.h>

#import "GFParameterCommitter.h"
#import "GFParameterIDs.h"

static const UInt32 kVisibleParameterIDs[] = {
    kGFFOV,
    kGFSmoothness,
    kGFLensCorrection,
    kGFHorizonLockAmount,
    kGFHorizonLockRoll,
    kGFZoomMode,
    kGFOverview,
};

static NSString *GFTimeKey(CMTime time) {
    return [NSString stringWithFormat:@"%lld/%d", time.value, time.timescale];
}

static NSError *GFTestError(NSString *message) {
    return [NSError errorWithDomain:@"test" code:1
                           userInfo:@{NSLocalizedDescriptionKey : message}];
}

@interface GFTestActionAPI : NSObject
@property(nonatomic) BOOL active;
@property(nonatomic) NSUInteger startCount;
@property(nonatomic) NSUInteger endCount;
@end

@implementation GFTestActionAPI
- (void)startAction:(id)sender {
    (void)sender;
    self.active = YES;
    self.startCount += 1;
}
- (void)endAction:(id)sender {
    (void)sender;
    self.active = NO;
    self.endCount += 1;
}
@end

@class GFTestKeyframeAPI;

@interface GFTestSettingAPI : NSObject
@property(nonatomic, weak) GFTestActionAPI *action;
@property(nonatomic, weak) GFTestKeyframeAPI *keyframes;
@property(nonatomic) NSUInteger outsideActionWrites;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSString *> *strings;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSNumber *> *baseValues;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSMutableDictionary<NSString *, NSNumber *> *> *animatedValues;
@property(nonatomic, strong) NSMutableArray<NSNumber *> *writeIDs;
@property(nonatomic) BOOL readsRequireAction;
@property(nonatomic) NSUInteger readCount;
@property(nonatomic) NSUInteger insideActionReads;
@property(nonatomic) BOOL discardVisibleWriteOnce;
@property(nonatomic) BOOL discardManifestWriteOnce;
@property(nonatomic) BOOL failProjectOverviewWriteOnce;
@property(nonatomic) BOOL discardOldFOVWriteOnce;
@property(nonatomic) BOOL projectStaticWritesAtZero;
@end

@interface GFTestKeyframeAPI : NSObject <FxKeyframeAPI_v3>
@property(nonatomic, weak) GFTestActionAPI *action;
@property(nonatomic, weak) GFTestSettingAPI *setting;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSMutableArray<NSValue *> *> *frames;
@property(nonatomic) BOOL failRemovalOnce;
- (NSUInteger)countForParameter:(UInt32)parameterID;
- (BOOL)hasKeyframeForParameter:(UInt32)parameterID atTime:(CMTime)time;
- (NSArray<NSValue *> *)keyframesForParameter:(UInt32)parameterID;
@end

@implementation GFTestSettingAPI
- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.strings = [NSMutableDictionary dictionary];
        self.baseValues = [NSMutableDictionary dictionary];
        self.animatedValues = [NSMutableDictionary dictionary];
        self.writeIDs = [NSMutableArray array];
        self.projectStaticWritesAtZero = YES;
    }
    return self;
}

- (BOOL)recordWriteForParameter:(UInt32)parameterID {
    if (!self.action.active) {
        self.outsideActionWrites += 1;
        return NO;
    }
    [self.writeIDs addObject:@(parameterID)];
    return YES;
}

- (BOOL)setStringParameterValue:(NSString *)value toParameter:(UInt32)parameterID {
    if (![self recordWriteForParameter:parameterID]) {
        return NO;
    }
    if ((parameterID == kGFProjectPayloadManifestA ||
         parameterID == kGFProjectPayloadManifestB) &&
        self.discardManifestWriteOnce) {
        self.discardManifestWriteOnce = NO;
        return YES;
    }
    self.strings[@(parameterID)] = value ?: @"";
    return YES;
}

- (BOOL)getStringParameterValue:(NSString **)value fromParameter:(UInt32)parameterID {
    self.readCount += 1;
    if (self.action.active) {
        self.insideActionReads += 1;
    }
    if (self.readsRequireAction && !self.action.active) {
        return NO;
    }
    if (value == NULL) {
        return NO;
    }
    *value = self.strings[@(parameterID)] ?: @"";
    return YES;
}

- (NSNumber *)numberForParameter:(UInt32)parameterID atTime:(CMTime)time {
    if ([self.keyframes hasKeyframeForParameter:parameterID atTime:time]) {
        NSNumber *animated = self.animatedValues[@(parameterID)][GFTimeKey(time)];
        if (animated != nil) {
            return animated;
        }
    }
    return self.baseValues[@(parameterID)] ?: @0;
}

- (BOOL)setNumber:(NSNumber *)value parameterID:(UInt32)parameterID atTime:(CMTime)time {
    if (![self recordWriteForParameter:parameterID]) {
        return NO;
    }
    if (parameterID == kGFSmoothness && value.doubleValue == 42.0 &&
        self.discardVisibleWriteOnce) {
        self.discardVisibleWriteOnce = NO;
        return YES;
    }
    BOOL isProjectStaticValue =
        (parameterID == kGFFOV && value.doubleValue == 1.25) ||
        (parameterID == kGFSmoothness && value.doubleValue == 42.0) ||
        (parameterID == kGFLensCorrection && value.doubleValue == 95.0) ||
        (parameterID == kGFHorizonLockAmount && value.doubleValue == 33.0) ||
        (parameterID == kGFHorizonLockRoll && value.doubleValue == 4.0) ||
        (parameterID == kGFZoomMode && value.intValue == 0) ||
        (parameterID == kGFOverview && !value.boolValue);
    if (isProjectStaticValue && CMTimeCompare(time, kCMTimeZero) != 0) {
        self.projectStaticWritesAtZero = NO;
    }
    if (parameterID == kGFOverview && !value.boolValue &&
        self.failProjectOverviewWriteOnce) {
        self.failProjectOverviewWriteOnce = NO;
        return NO;
    }
    if (parameterID == kGFFOV && value.doubleValue == 0.5 &&
        self.discardOldFOVWriteOnce) {
        self.discardOldFOVWriteOnce = NO;
        return YES;
    }
    if ([self.keyframes hasKeyframeForParameter:parameterID atTime:time]) {
        NSMutableDictionary<NSString *, NSNumber *> *values =
            self.animatedValues[@(parameterID)];
        if (values == nil) {
            values = [NSMutableDictionary dictionary];
            self.animatedValues[@(parameterID)] = values;
        }
        values[GFTimeKey(time)] = value;
    } else {
        self.baseValues[@(parameterID)] = value;
    }
    return YES;
}

- (BOOL)setFloatValue:(double)value toParameter:(UInt32)parameterID atTime:(CMTime)time {
    return [self setNumber:@(value) parameterID:parameterID atTime:time];
}
- (BOOL)setIntValue:(int)value toParameter:(UInt32)parameterID atTime:(CMTime)time {
    return [self setNumber:@(value) parameterID:parameterID atTime:time];
}
- (BOOL)setBoolValue:(BOOL)value toParameter:(UInt32)parameterID atTime:(CMTime)time {
    return [self setNumber:@(value) parameterID:parameterID atTime:time];
}
- (BOOL)getFloatValue:(double *)value fromParameter:(UInt32)parameterID atTime:(CMTime)time {
    if (value == NULL) {
        return NO;
    }
    self.readCount += 1;
    self.insideActionReads += self.action.active ? 1 : 0;
    *value = [self numberForParameter:parameterID atTime:time].doubleValue;
    return !self.readsRequireAction || self.action.active;
}
- (BOOL)getIntValue:(int *)value fromParameter:(UInt32)parameterID atTime:(CMTime)time {
    if (value == NULL) {
        return NO;
    }
    self.readCount += 1;
    self.insideActionReads += self.action.active ? 1 : 0;
    *value = [self numberForParameter:parameterID atTime:time].intValue;
    return !self.readsRequireAction || self.action.active;
}
- (BOOL)getBoolValue:(BOOL *)value fromParameter:(UInt32)parameterID atTime:(CMTime)time {
    if (value == NULL) {
        return NO;
    }
    self.readCount += 1;
    self.insideActionReads += self.action.active ? 1 : 0;
    *value = [self numberForParameter:parameterID atTime:time].boolValue;
    return !self.readsRequireAction || self.action.active;
}
@end

@implementation GFTestKeyframeAPI
- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.frames = [NSMutableDictionary dictionary];
    }
    return self;
}
- (NSMutableArray<NSValue *> *)mutableFramesForParameter:(NSUInteger)parameterID {
    NSMutableArray<NSValue *> *frames = self.frames[@(parameterID)];
    if (frames == nil) {
        frames = [NSMutableArray array];
        self.frames[@(parameterID)] = frames;
    }
    return frames;
}
- (NSArray<NSValue *> *)keyframesForParameter:(UInt32)parameterID {
    return [[self mutableFramesForParameter:parameterID] copy];
}
- (NSUInteger)countForParameter:(UInt32)parameterID {
    return [self mutableFramesForParameter:parameterID].count;
}
- (BOOL)hasKeyframeForParameter:(UInt32)parameterID atTime:(CMTime)time {
    for (NSValue *value in [self mutableFramesForParameter:parameterID]) {
        FxKeyframe frame;
        [value getValue:&frame size:sizeof(frame)];
        if (CMTimeCompare(frame.time, time) == 0) {
            return YES;
        }
    }
    return NO;
}
- (NSError *)channelCount:(NSUInteger *)count forParameter:(NSUInteger)parameterID {
    (void)parameterID;
    if (count != NULL) {
        *count = 1;
    }
    return nil;
}
- (NSError *)keyframeCount:(NSUInteger *)count
              forParameter:(NSUInteger)parameterID
                andChannel:(NSUInteger)channelIndex {
    if (count == NULL || channelIndex != 0) {
        return GFTestError(@"invalid keyframe count request");
    }
    *count = [self countForParameter:(UInt32)parameterID];
    return nil;
}
- (NSError *)keyframe:(FxKeyframe *)keyframe
         forParameter:(NSUInteger)parameterID
              channel:(NSUInteger)channelIndex
             andIndex:(NSUInteger)keyframeIndex {
    NSArray<NSValue *> *frames = [self mutableFramesForParameter:parameterID];
    if (keyframe == NULL || channelIndex != 0 || keyframeIndex >= frames.count) {
        return GFTestError(@"invalid keyframe request");
    }
    [frames[keyframeIndex] getValue:keyframe size:sizeof(*keyframe)];
    return nil;
}
- (NSError *)setKeyframeIndex:(NSUInteger)keyframeIndex
                 withKeyframe:(const FxKeyframe *)keyframe
                 forParameter:(NSUInteger)parameterID
                   andChannel:(NSUInteger)channelIndex {
    NSMutableArray<NSValue *> *frames = [self mutableFramesForParameter:parameterID];
    if (keyframe == NULL || channelIndex != 0 || keyframeIndex >= frames.count) {
        return GFTestError(@"invalid keyframe update");
    }
    frames[keyframeIndex] = [NSValue valueWithBytes:keyframe objCType:@encode(FxKeyframe)];
    return nil;
}
- (NSError *)addKeyframe:(const FxKeyframe *)keyframe
             toParameter:(NSUInteger)parameterID
              andChannel:(NSUInteger)channelIndex {
    if (keyframe == NULL || channelIndex != 0) {
        return GFTestError(@"invalid keyframe add");
    }
    [[self mutableFramesForParameter:parameterID]
        addObject:[NSValue valueWithBytes:keyframe objCType:@encode(FxKeyframe)]];
    return nil;
}
- (NSError *)removeKeyframeAtIndex:(NSUInteger)keyframeIndex
                     fromParameter:(NSUInteger)parameterID
                        andChannel:(NSUInteger)channelIndex {
    NSMutableArray<NSValue *> *frames = [self mutableFramesForParameter:parameterID];
    if (channelIndex != 0 || keyframeIndex >= frames.count) {
        return GFTestError(@"invalid keyframe remove");
    }
    [frames removeObjectAtIndex:keyframeIndex];
    return nil;
}
- (NSError *)removeAllKeyframesForParameter:(NSUInteger)parameterID
                                  andChannel:(NSUInteger)channelIndex {
    if (!self.action.active || channelIndex != 0) {
        return GFTestError(@"keyframe removal outside action");
    }
    if (self.failRemovalOnce) {
        self.failRemovalOnce = NO;
        return GFTestError(@"injected keyframe removal failure");
    }
    [[self mutableFramesForParameter:parameterID] removeAllObjects];
    [self.setting.animatedValues removeObjectForKey:@(parameterID)];
    return nil;
}
- (NSError *)parameter:(NSUInteger)parameterID
               channel:(NSUInteger)channelIndex
           hasKeyframe:(BOOL *)hasKeyframe
                atTime:(CMTime)time {
    if (channelIndex != 0 || hasKeyframe == NULL) {
        return GFTestError(@"invalid keyframe presence request");
    }
    *hasKeyframe = [self hasKeyframeForParameter:(UInt32)parameterID atTime:time];
    return nil;
}
- (NSError *)keyframe:(FxKeyframe *)keyframe
       atOrBeforeTime:(CMTime)time
        fromParameter:(NSUInteger)parameterID
           andChannel:(NSUInteger)channelIndex {
    if (keyframe == NULL || channelIndex != 0) {
        return GFTestError(@"invalid preceding keyframe request");
    }
    BOOL found = NO;
    for (NSValue *value in [self mutableFramesForParameter:parameterID]) {
        FxKeyframe candidate;
        [value getValue:&candidate size:sizeof(candidate)];
        if (CMTimeCompare(candidate.time, time) <= 0 &&
            (!found || CMTimeCompare(candidate.time, keyframe->time) > 0)) {
            *keyframe = candidate;
            found = YES;
        }
    }
    return found ? nil : GFTestError(@"no preceding keyframe");
}
- (NSError *)keyframe:(FxKeyframe *)keyframe
        atOrAfterTime:(CMTime)time
        fromParameter:(NSUInteger)parameterID
           andChannel:(NSUInteger)channelIndex {
    if (keyframe == NULL || channelIndex != 0) {
        return GFTestError(@"invalid following keyframe request");
    }
    BOOL found = NO;
    for (NSValue *value in [self mutableFramesForParameter:parameterID]) {
        FxKeyframe candidate;
        [value getValue:&candidate size:sizeof(candidate)];
        if (CMTimeCompare(candidate.time, time) >= 0 &&
            (!found || CMTimeCompare(candidate.time, keyframe->time) < 0)) {
            *keyframe = candidate;
            found = YES;
        }
    }
    return found ? nil : GFTestError(@"no following keyframe");
}
@end

@interface GFTestAPIManager : NSObject <PROAPIAccessing>
@property(nonatomic, strong) GFTestActionAPI *action;
@property(nonatomic, strong) GFTestSettingAPI *setting;
@property(nonatomic, strong) GFTestKeyframeAPI *keyframes;
@property(nonatomic) BOOL parameterAPIsRequireAction;
@end

@implementation GFTestAPIManager
- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.action = [[GFTestActionAPI alloc] init];
        self.setting = [[GFTestSettingAPI alloc] init];
        self.keyframes = [[GFTestKeyframeAPI alloc] init];
        self.setting.action = self.action;
        self.setting.keyframes = self.keyframes;
        self.keyframes.action = self.action;
        self.keyframes.setting = self.setting;
    }
    return self;
}
- (id)apiForProtocol:(Protocol *)apiProtocol {
    if (protocol_isEqual(apiProtocol, @protocol(FxCustomParameterActionAPI_v4))) {
        return self.action;
    }
    if (protocol_isEqual(apiProtocol, @protocol(FxParameterSettingAPI_v5)) ||
        protocol_isEqual(apiProtocol, @protocol(FxParameterRetrievalAPI_v6))) {
        return self.parameterAPIsRequireAction && !self.action.active ? nil : self.setting;
    }
    if (protocol_isEqual(apiProtocol, @protocol(FxKeyframeAPI_v3))) {
        return self.parameterAPIsRequireAction && !self.action.active ? nil : self.keyframes;
    }
    return nil;
}
@end

static GFRenderParameters GFProjectParameters(void) {
    return (GFRenderParameters){
        .fov = 1.25,
        .smoothness = 42.0,
        .lens_correction = 95.0,
        .horizon_lock_amount = 33.0,
        .horizon_lock_roll = 4.0,
        .zoom_mode = 0,
        .overview = 0,
        .reserved = {0, 0, 0},
    };
}

static GFRenderParameters GFOldParameters(void) {
    return (GFRenderParameters){
        .fov = 0.5,
        .smoothness = 9.0,
        .lens_correction = 80.0,
        .horizon_lock_amount = 3.0,
        .horizon_lock_roll = -2.0,
        .zoom_mode = 2,
        .overview = 1,
        .reserved = {0, 0, 0},
    };
}

static NSArray<NSNumber *> *GFParameterNumbers(GFRenderParameters parameters) {
    return @[
        @(parameters.fov),
        @(parameters.smoothness),
        @(parameters.lens_correction),
        @(parameters.horizon_lock_amount),
        @(parameters.horizon_lock_roll),
        @(parameters.zoom_mode),
        @(parameters.overview != 0),
    ];
}

static void GFSeedBaseValues(GFTestAPIManager *manager, GFRenderParameters parameters) {
    NSArray<NSNumber *> *numbers = GFParameterNumbers(parameters);
    for (NSUInteger index = 0; index < numbers.count; ++index) {
        manager.setting.baseValues[@(kVisibleParameterIDs[index])] = numbers[index];
    }
}

static void GFSeedKeyframes(GFTestAPIManager *manager) {
    for (NSUInteger parameterIndex = 0;
         parameterIndex < sizeof(kVisibleParameterIDs) / sizeof(kVisibleParameterIDs[0]);
         ++parameterIndex) {
        UInt32 parameterID = kVisibleParameterIDs[parameterIndex];
        NSMutableDictionary<NSString *, NSNumber *> *animated = [NSMutableDictionary dictionary];
        manager.setting.animatedValues[@(parameterID)] = animated;
        for (NSUInteger frameIndex = 0; frameIndex < 2; ++frameIndex) {
            FxKeyframe frame;
            FxInitKeyframe(frame, kFxKeyframe_CurrentVersion);
            frame.time = CMTimeMake((int64_t)(10 + frameIndex * 10), 1);
            frame.segmentStyle = frameIndex == 0
                ? kFxKeyframeSegmentStyle_Linear
                : kFxKeyframeSegmentStyle_Bezier;
            frame.inTangentX = (double)parameterIndex + 0.1 + frameIndex;
            frame.inTangentY = (double)parameterIndex + 0.2 + frameIndex;
            frame.outTangentX = (double)parameterIndex + 0.3 + frameIndex;
            frame.outTangentY = (double)parameterIndex + 0.4 + frameIndex;
            [[manager.keyframes mutableFramesForParameter:parameterID]
                addObject:[NSValue valueWithBytes:&frame objCType:@encode(FxKeyframe)]];
            NSNumber *value = parameterID == kGFOverview
                ? @(frameIndex == 0)
                : parameterID == kGFZoomMode
                    ? @((int)frameIndex)
                    : @(100.0 + parameterIndex * 10.0 + frameIndex);
            animated[GFTimeKey(frame.time)] = value;
        }
    }
}

static NSDictionary *GFSnapshotVisibleState(GFTestAPIManager *manager) {
    NSMutableDictionary *snapshot = [NSMutableDictionary dictionary];
    for (NSUInteger index = 0;
         index < sizeof(kVisibleParameterIDs) / sizeof(kVisibleParameterIDs[0]);
         ++index) {
        UInt32 parameterID = kVisibleParameterIDs[index];
        NSMutableArray *frames = [NSMutableArray array];
        for (NSValue *value in [manager.keyframes keyframesForParameter:parameterID]) {
            FxKeyframe frame;
            [value getValue:&frame size:sizeof(frame)];
            [frames addObject:@{
                @"version" : @(frame.version),
                @"time" : GFTimeKey(frame.time),
                @"style" : @(frame.segmentStyle),
                @"inX" : @(frame.inTangentX),
                @"inY" : @(frame.inTangentY),
                @"outX" : @(frame.outTangentX),
                @"outY" : @(frame.outTangentY),
                @"value" : [manager.setting numberForParameter:parameterID atTime:frame.time],
            }];
        }
        snapshot[[NSString stringWithFormat:@"%u", parameterID]] = @{
            @"base" : manager.setting.baseValues[@(parameterID)] ?: @0,
            @"frames" : frames,
        };
    }
    return snapshot;
}

static NSString *GFRepeatedPayload(NSString *quartet, NSUInteger length) {
    NSCAssert(quartet.length == 4 && length % 4 == 0, @"payload geometry");
    NSMutableString *payload = [NSMutableString stringWithCapacity:length];
    for (NSUInteger offset = 0; offset < length; offset += 4) {
        [payload appendString:quartet];
    }
    return payload;
}

static NSDictionary *GFDecodeManifest(NSString *encoded) {
    if (encoded.length == 0) {
        return @{};
    }
    NSData *json = [[NSData alloc] initWithBase64EncodedString:encoded options:0];
    id value = json == nil ? nil
        : [NSJSONSerialization JSONObjectWithData:json options:0 error:NULL];
    return [value isKindOfClass:[NSDictionary class]] ? value : @{};
}

static NSString *GFEncodeManifest(NSDictionary *manifest) {
    NSData *json = [NSJSONSerialization dataWithJSONObject:manifest
                                                   options:NSJSONWritingSortedKeys
                                                     error:NULL];
    return [json base64EncodedStringWithOptions:0];
}

static NSUInteger GFNonEmptyChunkCount(NSDictionary<NSNumber *, NSString *> *values,
                                       const UInt32 *chunkIDs) {
    NSUInteger count = 0;
    for (NSUInteger index = 0; index < kGFProjectPayloadChunksPerBank; ++index) {
        count += [values[@(chunkIDs[index])] length] > 0 ? 1 : 0;
    }
    return count;
}

static NSUInteger GFWriteCount(NSArray<NSNumber *> *writeIDs, UInt32 parameterID) {
    NSUInteger count = 0;
    for (NSNumber *writeID in writeIDs) {
        count += writeID.unsignedIntValue == parameterID ? 1 : 0;
    }
    return count;
}

static NSArray<NSNumber *> *GFManifestWriteIDs(NSArray<NSNumber *> *writeIDs) {
    NSMutableArray<NSNumber *> *result = [NSMutableArray array];
    for (NSNumber *writeID in writeIDs) {
        if (writeID.unsignedIntValue == kGFProjectPayloadManifestA ||
            writeID.unsignedIntValue == kGFProjectPayloadManifestB) {
            [result addObject:writeID];
        }
    }
    return result;
}

static void GFPrintJSON(NSDictionary *result) {
    NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                   options:NSJSONWritingSortedKeys
                                                     error:NULL];
    fwrite(json.bytes, 1, json.length, stdout);
    fputc('\n', stdout);
}

static BOOL GFCommit(GFParameterCommitter *committer,
                     NSString *payload,
                     NSString *displayName,
                     GFRenderParameters parameters,
                     id sender) {
    return [committer commitProjectPayload:payload
                               displayName:displayName
                                parameters:parameters
                                    sender:sender];
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        GFTestAPIManager *manager = [[GFTestAPIManager alloc] init];
        manager.setting.strings[@(kGFProjectPayload)] = @"previous-payload";
        GFParameterCommitter *committer =
            [[GFParameterCommitter alloc] initWithAPIManager:manager];
        GFRenderParameters parameters = GFProjectParameters();

        if (argc > 1 && strcmp(argv[1], "--cycle") == 0) {
            NSString *payloadA = GFRepeatedPayload(@"QUFB", 300000);
            NSString *payloadB = GFRepeatedPayload(@"QkJC", 300000);
            NSString *payloadC = GFRepeatedPayload(@"Q0ND", 300000);
            BOOL first = GFCommit(committer, payloadA, @"A.gyroflow", parameters, manager);
            BOOL second = GFCommit(committer, payloadB, @"B.gyroflow", parameters, manager);
            BOOL third = GFCommit(committer, payloadC, @"C.gyroflow", parameters, manager);
            NSError *latestError = nil;
            NSString *latest = [committer persistedProjectPayloadWithError:&latestError];
            manager.setting.strings[@(kGFProjectPayloadChunksA[0])] = @"corrupt";
            GFParameterCommitter *restarted =
                [[GFParameterCommitter alloc] initWithAPIManager:manager];
            NSError *fallbackError = nil;
            NSString *fallback = [restarted persistedProjectPayloadWithError:&fallbackError];
            NSDictionary *manifestA = GFDecodeManifest(
                manager.setting.strings[@(kGFProjectPayloadManifestA)]);
            NSDictionary *manifestB = GFDecodeManifest(
                manager.setting.strings[@(kGFProjectPayloadManifestB)]);
            GFPrintJSON(@{
                @"manifestWriteIDs" : GFManifestWriteIDs(manager.setting.writeIDs),
                @"generationA" : manifestA[@"generation"] ?: @0,
                @"generationB" : manifestB[@"generation"] ?: @0,
                @"latestRecovered" : @(latestError == nil && [latest isEqualToString:payloadC]),
                @"fallbackRecovered" : @(fallbackError == nil && [fallback isEqualToString:payloadB]),
                @"legacyWriteCount" : @(GFWriteCount(manager.setting.writeIDs, kGFProjectPayload)),
            });
            return first && second && third && latestError == nil && fallbackError == nil ? 0 : 1;
        }

        if (argc > 1 && strcmp(argv[1], "--conflict") == 0) {
            NSString *payloadA = GFRepeatedPayload(@"QUFB", 300000);
            NSString *payloadB = GFRepeatedPayload(@"QkJC", 300000);
            BOOL first = GFCommit(committer, payloadA, @"A.gyroflow", parameters, manager);
            BOOL second = GFCommit(committer, payloadB, @"B.gyroflow", parameters, manager);
            NSMutableDictionary *manifestA = [GFDecodeManifest(
                manager.setting.strings[@(kGFProjectPayloadManifestA)]) mutableCopy];
            manifestA[@"generation"] = @2;
            manager.setting.strings[@(kGFProjectPayloadManifestA)] = GFEncodeManifest(manifestA);
            GFParameterCommitter *restarted =
                [[GFParameterCommitter alloc] initWithAPIManager:manager];
            NSError *error = nil;
            NSString *recovered = [restarted persistedProjectPayloadWithError:&error];
            GFPrintJSON(@{@"recovered" : @(recovered != nil),
                          @"error" : error.localizedDescription ?: @""});
            return first && second && recovered == nil && error != nil ? 0 : 1;
        }

        if (argc > 1 && strcmp(argv[1], "--capacity") == 0) {
            NSString *maximum = GFRepeatedPayload(@"QUFB", kGFProjectPayloadMaximumBytes);
            BOOL maximumCommitted = GFCommit(
                committer, maximum, @"maximum.gyroflow", parameters, manager);
            NSUInteger startCountAfterMaximum = manager.action.startCount;
            NSString *oversized = [maximum stringByAppendingString:@"A"];
            BOOL oversizedCommitted = GFCommit(
                committer, oversized, @"oversized.gyroflow", parameters, manager);
            GFPrintJSON(@{
                @"maximumCommitted" : @(maximumCommitted),
                @"oversizedCommitted" : @(oversizedCommitted),
                @"startCountAfterMaximum" : @(startCountAfterMaximum),
                @"startCountAfterOversized" : @(manager.action.startCount),
                @"maximumUsedChunks" : @(GFNonEmptyChunkCount(
                    manager.setting.strings, kGFProjectPayloadChunksA)),
            });
            return maximumCommitted && !oversizedCommitted ? 0 : 1;
        }

        NSString *oldPayload = @"b2xkLXByb2plY3Q=";
        BOOL oldCommitted = GFCommit(
            committer, oldPayload, @"old.gyroflow", GFOldParameters(), manager);
        NSCAssert(oldCommitted, @"seed previous selected bank");
        [manager.setting.writeIDs removeAllObjects];
        manager.action.startCount = 0;
        manager.action.endCount = 0;
        manager.setting.readCount = 0;
        manager.setting.insideActionReads = 0;
        manager.setting.projectStaticWritesAtZero = YES;
        GFSeedBaseValues(manager, GFOldParameters());
        manager.setting.strings[@(kGFProjectDisplayName)] = @"old.gyroflow";
        GFSeedKeyframes(manager);
        NSDictionary *oldVisibleState = GFSnapshotVisibleState(manager);

        if (argc > 1 && strcmp(argv[1], "--discard-visible-write") == 0) {
            manager.setting.discardVisibleWriteOnce = YES;
        } else if (argc > 1 && strcmp(argv[1], "--discard-manifest-write") == 0) {
            manager.setting.discardManifestWriteOnce = YES;
        } else if (argc > 1 && strcmp(argv[1], "--fail-keyframe-removal") == 0) {
            manager.keyframes.failRemovalOnce = YES;
        } else if (argc > 1 && strcmp(argv[1], "--discard-rollback-write") == 0) {
            manager.setting.failProjectOverviewWriteOnce = YES;
            manager.setting.discardOldFOVWriteOnce = YES;
        } else if (argc > 1 && strcmp(argv[1], "--action-gated-apis") == 0) {
            manager.parameterAPIsRequireAction = YES;
            manager.setting.readsRequireAction = YES;
        }

        NSString *payload = @"dmVyc2lvbmVkLWJhc2U2NC1lbnZlbG9wZQ==";
        BOOL committed = GFCommit(
            committer, payload, @"P1004783.gyroflow", parameters, manager);
        NSError *recoveryError = nil;
        NSString *recovered = [committer persistedProjectPayloadWithError:&recoveryError];
        NSDictionary *manifestB = GFDecodeManifest(
            manager.setting.strings[@(kGFProjectPayloadManifestB)]);
        NSDictionary *newVisibleState = GFSnapshotVisibleState(manager);
        NSUInteger totalKeyframes = 0;
        for (NSUInteger index = 0;
             index < sizeof(kVisibleParameterIDs) / sizeof(kVisibleParameterIDs[0]);
             ++index) {
            totalKeyframes += [manager.keyframes countForParameter:kVisibleParameterIDs[index]];
        }
        GFPrintJSON(@{
            @"committed" : @(committed),
            @"startCount" : @(manager.action.startCount),
            @"endCount" : @(manager.action.endCount),
            @"outsideActionWrites" : @(manager.setting.outsideActionWrites),
            @"payload" : payload,
            @"recoveredPayload" : recovered ?: @"",
            @"recoveryError" : recoveryError.localizedDescription ?: @"",
            @"readCount" : @(manager.setting.readCount),
            @"insideActionReads" : @(manager.setting.insideActionReads),
            @"lastWriteID" : manager.setting.writeIDs.lastObject ?: @0,
            @"legacyWriteCount" : @(GFWriteCount(manager.setting.writeIDs, kGFProjectPayload)),
            @"nonEmptyChunksB" : @(GFNonEmptyChunkCount(
                manager.setting.strings, kGFProjectPayloadChunksB)),
            @"manifestB" : manifestB,
            @"displayName" : manager.setting.strings[@(kGFProjectDisplayName)] ?: @"",
            @"visibleState" : newVisibleState,
            @"oldVisibleState" : oldVisibleState,
            @"totalKeyframes" : @(totalKeyframes),
            @"rollbackRestored" : @([newVisibleState isEqual:oldVisibleState]),
            @"manifestWriteIDs" : GFManifestWriteIDs(manager.setting.writeIDs),
            @"projectStaticWritesAtZero" : @(manager.setting.projectStaticWritesAtZero),
        });
        return committed ? 0 : 1;
    }
}
