#import "GFParameterCommitter.h"

#import "GFParameterIDs.h"
#import <CommonCrypto/CommonDigest.h>
#import <os/log.h>

static const NSUInteger kGFProjectPayloadManifestVersion = 1;
static NSString *const GFProjectPayloadErrorDomain =
    @"com.niyien.gyroflow.finalcut.project-payload";

typedef NS_ENUM(NSUInteger, GFParameterValueKind) {
    GFParameterValueKindFloat,
    GFParameterValueKindInt,
    GFParameterValueKindBool,
};

typedef struct GFParameterSpec {
    UInt32 parameterID;
    GFParameterValueKind kind;
} GFParameterSpec;

static const GFParameterSpec kGFProjectParameterSpecs[] = {
    {kGFFOV, GFParameterValueKindFloat},
    {kGFSmoothness, GFParameterValueKindFloat},
    {kGFLensCorrection, GFParameterValueKindFloat},
    {kGFHorizonLockAmount, GFParameterValueKindFloat},
    {kGFHorizonLockRoll, GFParameterValueKindFloat},
    {kGFZoomMode, GFParameterValueKindInt},
    {kGFOverview, GFParameterValueKindBool},
};

static const NSUInteger kGFProjectParameterCount =
    sizeof(kGFProjectParameterSpecs) / sizeof(kGFProjectParameterSpecs[0]);

static NSError *GFProjectPayloadError(NSString *message) {
    return [NSError errorWithDomain:GFProjectPayloadErrorDomain
                               code:1
                           userInfo:@{NSLocalizedDescriptionKey : message}];
}

static NSString *GFSHA256(NSData *data) {
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(data.bytes, (CC_LONG)data.length, digest);
    NSMutableString *result =
        [NSMutableString stringWithCapacity:CC_SHA256_DIGEST_LENGTH * 2];
    for (NSUInteger index = 0; index < CC_SHA256_DIGEST_LENGTH; ++index) {
        [result appendFormat:@"%02x", digest[index]];
    }
    return result;
}

static BOOL GFUnsignedInteger(NSDictionary *dictionary,
                              NSString *key,
                              unsigned long long *value) {
    id candidate = dictionary[key];
    if (![candidate isKindOfClass:[NSNumber class]] ||
        CFGetTypeID((__bridge CFTypeRef)candidate) == CFBooleanGetTypeID()) {
        return NO;
    }
    double floating = [candidate doubleValue];
    long long signedValue = [candidate longLongValue];
    if (!isfinite(floating) || floating != floor(floating) || signedValue <= 0) {
        return NO;
    }
    *value = [candidate unsignedLongLongValue];
    return YES;
}

static BOOL GFValidLowercaseSHA256(NSString *value) {
    if (![value isKindOfClass:[NSString class]] || value.length != 64) {
        return NO;
    }
    NSCharacterSet *allowed = [NSCharacterSet characterSetWithCharactersInString:
        @"0123456789abcdef"];
    return [value rangeOfCharacterFromSet:allowed.invertedSet].location == NSNotFound;
}

@interface GFParameterCommitter ()
@property(nonatomic, strong) id<PROAPIAccessing> apiManager;
@property(nonatomic, strong) NSLock *cacheLock;
@property(nonatomic, copy) NSString *cachedManifestA;
@property(nonatomic, copy) NSString *cachedManifestB;
@property(nonatomic, copy) NSString *cachedPayload;
@property(nonatomic, copy) NSString *cachedPayloadHash;
@end

@interface GFParameterSnapshot : NSObject
@property(nonatomic) UInt32 parameterID;
@property(nonatomic) GFParameterValueKind kind;
@property(nonatomic, strong) NSNumber *baseValue;
@property(nonatomic, copy) NSArray<NSValue *> *keyframes;
@property(nonatomic, copy) NSArray<NSNumber *> *keyframeValues;
@end

@implementation GFParameterSnapshot
@end

@implementation GFParameterCommitter

- (instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager {
    self = [super init];
    if (self != nil) {
        self.apiManager = apiManager;
        self.cacheLock = [[NSLock alloc] init];
    }
    return self;
}

- (BOOL)readString:(NSString **)value
       parameterID:(UInt32)parameterID
          retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval {
    NSString *readValue = nil;
    if (![retrieval getStringParameterValue:&readValue fromParameter:parameterID]) {
        return NO;
    }
    if (value != NULL) {
        *value = readValue ?: @"";
    }
    return YES;
}

- (nullable NSDictionary *)candidateForManifest:(NSString *)encodedManifest
                                        chunkIDs:(const UInt32 *)chunkIDs
                                       retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval
                                            bank:(NSString *)bank
                                           error:(NSError **)error {
    if (encodedManifest.length == 0) {
        return nil;
    }
    NSData *manifestData =
        [[NSData alloc] initWithBase64EncodedString:encodedManifest options:0];
    if (manifestData == nil) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ manifest Base64 is invalid",
                                           bank]);
        }
        return nil;
    }
    id manifestValue = [NSJSONSerialization JSONObjectWithData:manifestData
                                                       options:0
                                                         error:NULL];
    if (![manifestValue isKindOfClass:[NSDictionary class]]) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ manifest JSON is invalid",
                                           bank]);
        }
        return nil;
    }
    NSDictionary *manifest = manifestValue;
    unsigned long long version = 0;
    unsigned long long generation = 0;
    unsigned long long chunkCount = 0;
    unsigned long long encodedLength = 0;
    if (!GFUnsignedInteger(manifest, @"version", &version) ||
        version != kGFProjectPayloadManifestVersion) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ manifest version is unknown",
                                           bank]);
        }
        return nil;
    }
    if (!GFUnsignedInteger(manifest, @"generation", &generation) ||
        !GFUnsignedInteger(manifest, @"chunk_count", &chunkCount) ||
        chunkCount > kGFProjectPayloadChunksPerBank ||
        !GFUnsignedInteger(manifest, @"encoded_length", &encodedLength) ||
        encodedLength > kGFProjectPayloadMaximumBytes) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ manifest bounds are invalid",
                                           bank]);
        }
        return nil;
    }
    NSUInteger expectedChunkCount =
        ((NSUInteger)encodedLength + kGFProjectPayloadChunkBytes - 1) /
        kGFProjectPayloadChunkBytes;
    if (chunkCount != expectedChunkCount ||
        !GFValidLowercaseSHA256(manifest[@"payload_sha256"])) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ manifest geometry is invalid",
                                           bank]);
        }
        return nil;
    }

    NSMutableData *payloadData = [NSMutableData dataWithCapacity:(NSUInteger)encodedLength];
    for (NSUInteger index = 0; index < (NSUInteger)chunkCount; ++index) {
        NSString *chunk = nil;
        if (![self readString:&chunk parameterID:chunkIDs[index] retrieval:retrieval]) {
            if (error != NULL) {
                *error = GFProjectPayloadError(
                    [NSString stringWithFormat:@"project payload bank %@ chunk %lu is unreadable",
                                               bank,
                                               (unsigned long)(index + 1)]);
            }
            return nil;
        }
        NSData *chunkData = [chunk dataUsingEncoding:NSASCIIStringEncoding
                                allowLossyConversion:NO];
        NSUInteger expectedLength = index + 1 < (NSUInteger)chunkCount
            ? kGFProjectPayloadChunkBytes
            : (NSUInteger)encodedLength -
                (kGFProjectPayloadChunkBytes * ((NSUInteger)chunkCount - 1));
        if (chunkData == nil || chunkData.length != expectedLength) {
            if (error != NULL) {
                *error = GFProjectPayloadError(
                    [NSString stringWithFormat:@"project payload bank %@ chunk %lu length is invalid",
                                               bank,
                                               (unsigned long)(index + 1)]);
            }
            return nil;
        }
        [payloadData appendData:chunkData];
    }
    if (payloadData.length != (NSUInteger)encodedLength ||
        ![GFSHA256(payloadData) isEqualToString:manifest[@"payload_sha256"]]) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ hash is invalid", bank]);
        }
        return nil;
    }
    NSString *payload = [[NSString alloc] initWithData:payloadData
                                               encoding:NSASCIIStringEncoding];
    if (payload == nil ||
        [[NSData alloc] initWithBase64EncodedString:payload options:0] == nil) {
        if (error != NULL) {
            *error = GFProjectPayloadError(
                [NSString stringWithFormat:@"project payload bank %@ content Base64 is invalid",
                                           bank]);
        }
        return nil;
    }
    return @{
        @"bank" : bank,
        @"generation" : @(generation),
        @"payload" : payload,
        @"payloadHash" : manifest[@"payload_sha256"],
    };
}

- (nullable NSDictionary *)preferredCandidateA:(nullable NSDictionary *)candidateA
                                      candidateB:(nullable NSDictionary *)candidateB
                                           error:(NSError **)error {
    if (candidateA == nil) {
        return candidateB;
    }
    if (candidateB == nil) {
        return candidateA;
    }
    unsigned long long generationA = [candidateA[@"generation"] unsignedLongLongValue];
    unsigned long long generationB = [candidateB[@"generation"] unsignedLongLongValue];
    if (generationA == generationB) {
        if (![candidateA[@"payload"] isEqualToString:candidateB[@"payload"]]) {
            if (error != NULL) {
                *error = GFProjectPayloadError(
                    @"project payload banks have the same generation with different content");
            }
            return nil;
        }
        return candidateA;
    }
    return generationA > generationB ? candidateA : candidateB;
}

- (nullable NSString *)persistedProjectPayloadWithError:(NSError **)error {
    return [self persistedProjectPayloadWithHash:NULL error:error];
}

- (nullable NSString *)persistedProjectPayloadWithHash:(NSString **)hash
                                                  error:(NSError **)error {
    if (hash != NULL) {
        *hash = nil;
    }
    if (error != NULL) {
        *error = nil;
    }
    id<FxParameterRetrievalAPI_v6> retrieval =
        [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
    if (retrieval == nil) {
        if (error != NULL) {
            *error = GFProjectPayloadError(@"project payload retrieval API is unavailable");
        }
        return nil;
    }
    NSString *manifestA = nil;
    NSString *manifestB = nil;
    if (![self readString:&manifestA
              parameterID:kGFProjectPayloadManifestA
                 retrieval:retrieval] ||
        ![self readString:&manifestB
              parameterID:kGFProjectPayloadManifestB
                 retrieval:retrieval]) {
        if (error != NULL) {
            *error = GFProjectPayloadError(@"project payload manifests are unreadable");
        }
        return nil;
    }

    [self.cacheLock lock];
    if (self.cachedPayload != nil &&
        [self.cachedManifestA isEqualToString:manifestA] &&
        [self.cachedManifestB isEqualToString:manifestB]) {
        NSString *cached = self.cachedPayload;
        if (hash != NULL) {
            *hash = self.cachedPayloadHash;
        }
        [self.cacheLock unlock];
        return cached;
    }
    [self.cacheLock unlock];

    NSError *bankAError = nil;
    NSError *bankBError = nil;
    NSDictionary *candidateA =
        [self candidateForManifest:manifestA
                          chunkIDs:kGFProjectPayloadChunksA
                         retrieval:retrieval
                              bank:@"A"
                             error:&bankAError];
    NSDictionary *candidateB =
        [self candidateForManifest:manifestB
                          chunkIDs:kGFProjectPayloadChunksB
                         retrieval:retrieval
                              bank:@"B"
                             error:&bankBError];
    NSError *selectionError = nil;
    NSDictionary *selected = [self preferredCandidateA:candidateA
                                            candidateB:candidateB
                                                 error:&selectionError];
    if (selectionError != nil) {
        if (error != NULL) {
            *error = selectionError;
        }
        return nil;
    }
    NSString *payload = selected[@"payload"];
    if (payload == nil) {
        NSString *legacy = nil;
        if (![self readString:&legacy
                  parameterID:kGFProjectPayload
                     retrieval:retrieval]) {
            if (error != NULL) {
                *error = GFProjectPayloadError(@"legacy project payload is unreadable");
            }
            return nil;
        }
        if (legacy.length > 0) {
            if (hash != NULL) {
                NSData *legacyData = [legacy dataUsingEncoding:NSASCIIStringEncoding];
                *hash = GFSHA256(legacyData ?: [NSData data]);
            }
            return legacy;
        }
        if (bankAError != nil || bankBError != nil) {
            if (error != NULL) {
                *error = bankAError ?: bankBError;
            }
            return nil;
        }
        if (hash != NULL) {
            *hash = @"";
        }
        return @"";
    }

    NSString *payloadHash = selected[@"payloadHash"];

    [self.cacheLock lock];
    self.cachedManifestA = manifestA;
    self.cachedManifestB = manifestB;
    self.cachedPayload = payload;
    self.cachedPayloadHash = payloadHash;
    [self.cacheLock unlock];
    if (hash != NULL) {
        *hash = payloadHash;
    }
    return payload;
}

- (nullable NSNumber *)numberForSpec:(GFParameterSpec)spec
                              atTime:(CMTime)time
                           retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval {
    switch (spec.kind) {
        case GFParameterValueKindFloat: {
            double value = 0.0;
            return [retrieval getFloatValue:&value
                               fromParameter:spec.parameterID
                                      atTime:time]
                ? @(value)
                : nil;
        }
        case GFParameterValueKindInt: {
            int value = 0;
            return [retrieval getIntValue:&value
                             fromParameter:spec.parameterID
                                    atTime:time]
                ? @(value)
                : nil;
        }
        case GFParameterValueKindBool: {
            BOOL value = NO;
            return [retrieval getBoolValue:&value
                              fromParameter:spec.parameterID
                                     atTime:time]
                ? @(value)
                : nil;
        }
    }
}

- (BOOL)setNumber:(NSNumber *)number
           forSpec:(GFParameterSpec)spec
            atTime:(CMTime)time
           setting:(id<FxParameterSettingAPI_v5>)setting {
    switch (spec.kind) {
        case GFParameterValueKindFloat:
            return [setting setFloatValue:number.doubleValue
                              toParameter:spec.parameterID
                                   atTime:time];
        case GFParameterValueKindInt:
            return [setting setIntValue:number.intValue
                            toParameter:spec.parameterID
                                 atTime:time];
        case GFParameterValueKindBool:
            return [setting setBoolValue:number.boolValue
                             toParameter:spec.parameterID
                                  atTime:time];
    }
}

- (BOOL)number:(NSNumber *)actual
    equalsExpected:(NSNumber *)expected
              kind:(GFParameterValueKind)kind {
    switch (kind) {
        case GFParameterValueKindFloat:
            return actual.doubleValue == expected.doubleValue;
        case GFParameterValueKindInt:
            return actual.intValue == expected.intValue;
        case GFParameterValueKindBool:
            return actual.boolValue == expected.boolValue;
    }
}

- (nullable NSArray<GFParameterSnapshot *> *)parameterSnapshotsWithRetrieval:
        (id<FxParameterRetrievalAPI_v6>)retrieval
                                                                  keyframes:
        (id<FxKeyframeAPI_v3>)keyframes {
    NSMutableArray<GFParameterSnapshot *> *snapshots =
        [NSMutableArray arrayWithCapacity:kGFProjectParameterCount];
    for (NSUInteger index = 0; index < kGFProjectParameterCount; ++index) {
        GFParameterSpec spec = kGFProjectParameterSpecs[index];
        NSNumber *baseValue = [self numberForSpec:spec
                                          atTime:kCMTimeZero
                                       retrieval:retrieval];
        NSUInteger channelCount = 0;
        if (baseValue == nil ||
            [keyframes channelCount:&channelCount forParameter:spec.parameterID] != nil ||
            channelCount != 1) {
            return nil;
        }
        NSUInteger keyframeCount = 0;
        if ([keyframes keyframeCount:&keyframeCount
                        forParameter:spec.parameterID
                          andChannel:0] != nil) {
            return nil;
        }
        NSMutableArray<NSValue *> *snapshotKeyframes =
            [NSMutableArray arrayWithCapacity:keyframeCount];
        NSMutableArray<NSNumber *> *snapshotValues =
            [NSMutableArray arrayWithCapacity:keyframeCount];
        for (NSUInteger keyframeIndex = 0;
             keyframeIndex < keyframeCount;
             ++keyframeIndex) {
            FxKeyframe keyframe;
            FxInitKeyframe(keyframe, kFxKeyframe_CurrentVersion);
            if ([keyframes keyframe:&keyframe
                       forParameter:spec.parameterID
                            channel:0
                           andIndex:keyframeIndex] != nil) {
                return nil;
            }
            NSNumber *keyframeValue = [self numberForSpec:spec
                                                   atTime:keyframe.time
                                                retrieval:retrieval];
            if (keyframeValue == nil) {
                return nil;
            }
            [snapshotKeyframes addObject:
                [NSValue valueWithBytes:&keyframe objCType:@encode(FxKeyframe)]];
            [snapshotValues addObject:keyframeValue];
        }
        GFParameterSnapshot *snapshot = [[GFParameterSnapshot alloc] init];
        snapshot.parameterID = spec.parameterID;
        snapshot.kind = spec.kind;
        snapshot.baseValue = baseValue;
        snapshot.keyframes = snapshotKeyframes;
        snapshot.keyframeValues = snapshotValues;
        [snapshots addObject:snapshot];
    }
    return snapshots;
}

- (BOOL)removeAllProjectKeyframes:(id<FxKeyframeAPI_v3>)keyframes {
    BOOL removed = YES;
    for (NSUInteger index = 0; index < kGFProjectParameterCount; ++index) {
        GFParameterSpec spec = kGFProjectParameterSpecs[index];
        if ([keyframes removeAllKeyframesForParameter:spec.parameterID
                                          andChannel:0] != nil) {
            removed = NO;
        }
    }
    if (!removed) {
        return NO;
    }
    for (NSUInteger index = 0; index < kGFProjectParameterCount; ++index) {
        NSUInteger keyframeCount = 0;
        if ([keyframes keyframeCount:&keyframeCount
                        forParameter:kGFProjectParameterSpecs[index].parameterID
                          andChannel:0] != nil ||
            keyframeCount != 0) {
            return NO;
        }
    }
    return YES;
}

- (BOOL)keyframe:(FxKeyframe)actual equalsKeyframe:(FxKeyframe)expected {
    return actual.version == expected.version &&
        CMTimeCompare(actual.time, expected.time) == 0 &&
        actual.segmentStyle == expected.segmentStyle &&
        actual.inTangentX == expected.inTangentX &&
        actual.inTangentY == expected.inTangentY &&
        actual.outTangentX == expected.outTangentX &&
        actual.outTangentY == expected.outTangentY;
}

- (BOOL)parametersMatchSnapshots:(NSArray<GFParameterSnapshot *> *)snapshots
                        retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval
                         keyframes:(id<FxKeyframeAPI_v3>)keyframes {
    for (GFParameterSnapshot *snapshot in snapshots) {
        GFParameterSpec spec = {snapshot.parameterID, snapshot.kind};
        NSNumber *baseValue = [self numberForSpec:spec
                                          atTime:kCMTimeZero
                                       retrieval:retrieval];
        if (baseValue == nil ||
            ![self number:baseValue
              equalsExpected:snapshot.baseValue
                        kind:snapshot.kind]) {
            return NO;
        }
        NSUInteger keyframeCount = 0;
        if ([keyframes keyframeCount:&keyframeCount
                        forParameter:snapshot.parameterID
                          andChannel:0] != nil ||
            keyframeCount != snapshot.keyframes.count) {
            return NO;
        }
        for (NSUInteger index = 0; index < keyframeCount; ++index) {
            FxKeyframe expected;
            [snapshot.keyframes[index] getValue:&expected size:sizeof(expected)];
            FxKeyframe actual;
            FxInitKeyframe(actual, kFxKeyframe_CurrentVersion);
            if ([keyframes keyframe:&actual
                       forParameter:snapshot.parameterID
                            channel:0
                           andIndex:index] != nil ||
                ![self keyframe:actual equalsKeyframe:expected]) {
                return NO;
            }
            NSNumber *actualValue = [self numberForSpec:spec
                                                 atTime:actual.time
                                              retrieval:retrieval];
            if (actualValue == nil ||
                ![self number:actualValue
                  equalsExpected:snapshot.keyframeValues[index]
                            kind:snapshot.kind]) {
                return NO;
            }
        }
    }
    return YES;
}

- (BOOL)staticParametersMatch:(GFRenderParameters)parameters
                     retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval {
    NSArray<NSNumber *> *expected = @[
        @(parameters.fov),
        @(parameters.smoothness),
        @(parameters.lens_correction),
        @(parameters.horizon_lock_amount),
        @(parameters.horizon_lock_roll),
        @(parameters.zoom_mode),
        @(parameters.overview != 0),
    ];
    for (NSUInteger index = 0; index < kGFProjectParameterCount; ++index) {
        GFParameterSpec spec = kGFProjectParameterSpecs[index];
        NSNumber *actual = [self numberForSpec:spec
                                       atTime:kCMTimeZero
                                    retrieval:retrieval];
        if (actual == nil ||
            ![self number:actual equalsExpected:expected[index] kind:spec.kind]) {
            return NO;
        }
    }
    return YES;
}

- (BOOL)setStaticParameters:(GFRenderParameters)parameters
                      setting:(id<FxParameterSettingAPI_v5>)setting {
    NSArray<NSNumber *> *values = @[
        @(parameters.fov),
        @(parameters.smoothness),
        @(parameters.lens_correction),
        @(parameters.horizon_lock_amount),
        @(parameters.horizon_lock_roll),
        @(parameters.zoom_mode),
        @(parameters.overview != 0),
    ];
    for (NSUInteger index = 0; index < kGFProjectParameterCount; ++index) {
        if (![self setNumber:values[index]
                     forSpec:kGFProjectParameterSpecs[index]
                      atTime:kCMTimeZero
                     setting:setting]) {
            return NO;
        }
    }
    return YES;
}

- (BOOL)restoreSnapshots:(NSArray<GFParameterSnapshot *> *)snapshots
                 chunkIDs:(const UInt32 *)chunkIDs
               oldChunks:(NSArray<NSString *> *)oldChunks
               manifestID:(UInt32)manifestID
              oldManifest:(NSString *)oldManifest
           oldDisplayName:(NSString *)oldDisplayName
                 retrieval:(id<FxParameterRetrievalAPI_v6>)retrieval
                   setting:(id<FxParameterSettingAPI_v5>)setting
                 keyframes:(id<FxKeyframeAPI_v3>)keyframes {
    for (NSUInteger attempt = 0; attempt < 2; ++attempt) {
        BOOL restored = [self removeAllProjectKeyframes:keyframes];
        for (GFParameterSnapshot *snapshot in snapshots) {
            GFParameterSpec spec = {snapshot.parameterID, snapshot.kind};
            restored = [self setNumber:snapshot.baseValue
                               forSpec:spec
                                atTime:kCMTimeZero
                               setting:setting] && restored;
            for (NSUInteger index = 0; index < snapshot.keyframes.count; ++index) {
                FxKeyframe keyframe;
                [snapshot.keyframes[index] getValue:&keyframe size:sizeof(keyframe)];
                restored = [keyframes addKeyframe:&keyframe
                                       toParameter:snapshot.parameterID
                                        andChannel:0] == nil && restored;
                restored = [self setNumber:snapshot.keyframeValues[index]
                                   forSpec:spec
                                    atTime:keyframe.time
                                   setting:setting] && restored;
            }
        }
        for (NSUInteger index = 0; index < kGFProjectPayloadChunksPerBank; ++index) {
            restored = [setting setStringParameterValue:oldChunks[index]
                                            toParameter:chunkIDs[index]] && restored;
        }
        restored = [setting setStringParameterValue:oldDisplayName
                                        toParameter:kGFProjectDisplayName] && restored;
        restored = [setting setStringParameterValue:oldManifest
                                        toParameter:manifestID] && restored;

        BOOL exact = restored &&
            [self parametersMatchSnapshots:snapshots
                                  retrieval:retrieval
                                   keyframes:keyframes];
        for (NSUInteger index = 0;
             index < kGFProjectPayloadChunksPerBank && exact;
             ++index) {
            NSString *value = nil;
            exact = [self readString:&value
                         parameterID:chunkIDs[index]
                            retrieval:retrieval] &&
                [value isEqualToString:oldChunks[index]];
        }
        NSString *displayName = nil;
        NSString *manifest = nil;
        exact = exact &&
            [self readString:&displayName
                 parameterID:kGFProjectDisplayName
                    retrieval:retrieval] &&
            [displayName isEqualToString:oldDisplayName] &&
            [self readString:&manifest
                 parameterID:manifestID
                    retrieval:retrieval] &&
            [manifest isEqualToString:oldManifest];
        if (exact) {
            return YES;
        }
    }
    return NO;
}

- (nullable NSString *)manifestForPayloadData:(NSData *)payloadData
                                    generation:(unsigned long long)generation
                                    chunkCount:(NSUInteger)chunkCount {
    NSDictionary *manifest = @{
        @"version" : @(kGFProjectPayloadManifestVersion),
        @"generation" : @(generation),
        @"chunk_count" : @(chunkCount),
        @"encoded_length" : @(payloadData.length),
        @"payload_sha256" : GFSHA256(payloadData),
    };
    NSData *json = [NSJSONSerialization dataWithJSONObject:manifest
                                                   options:NSJSONWritingSortedKeys
                                                     error:NULL];
    return json == nil ? nil : [json base64EncodedStringWithOptions:0];
}

- (BOOL)commitProjectPayload:(NSString *)projectPayload
                 displayName:(NSString *)displayName
                  parameters:(GFRenderParameters)parameters
                      sender:(id)sender {
    if (projectPayload.length == 0 || displayName.length == 0 || sender == nil ||
        ![displayName.lastPathComponent isEqualToString:displayName]) {
        return NO;
    }
    NSData *payloadData = [projectPayload dataUsingEncoding:NSASCIIStringEncoding
                                      allowLossyConversion:NO];
    if (payloadData == nil || payloadData.length > kGFProjectPayloadMaximumBytes) {
        os_log_error(OS_LOG_DEFAULT,
                     "Gyroflow project payload rejected: encoded payload exceeds 4 MiB");
        return NO;
    }
    if ([[NSData alloc] initWithBase64EncodedString:projectPayload options:0] == nil) {
        os_log_error(OS_LOG_DEFAULT,
                     "Gyroflow project payload rejected: payload is not strict Base64");
        return NO;
    }
    NSUInteger chunkCount =
        (payloadData.length + kGFProjectPayloadChunkBytes - 1) /
        kGFProjectPayloadChunkBytes;
    NSMutableArray<NSString *> *chunks =
        [NSMutableArray arrayWithCapacity:kGFProjectPayloadChunksPerBank];
    for (NSUInteger index = 0; index < kGFProjectPayloadChunksPerBank; ++index) {
        if (index < chunkCount) {
            NSRange range = NSMakeRange(
                index * kGFProjectPayloadChunkBytes,
                MIN(kGFProjectPayloadChunkBytes,
                    payloadData.length - index * kGFProjectPayloadChunkBytes));
            NSData *chunkData = [payloadData subdataWithRange:range];
            NSString *chunk = [[NSString alloc] initWithData:chunkData
                                                    encoding:NSASCIIStringEncoding];
            if (chunk == nil) {
                return NO;
            }
            [chunks addObject:chunk];
        } else {
            [chunks addObject:@""];
        }
    }

    id<FxCustomParameterActionAPI_v4> action =
        [self.apiManager apiForProtocol:@protocol(FxCustomParameterActionAPI_v4)];
    if (action == nil) {
        os_log_error(OS_LOG_DEFAULT,
                     "Gyroflow project payload rejected: action API unavailable");
        return NO;
    }
    [self.cacheLock lock];
    self.cachedManifestA = nil;
    self.cachedManifestB = nil;
    self.cachedPayload = nil;
    self.cachedPayloadHash = nil;
    [self.cacheLock unlock];

    id<FxParameterRetrievalAPI_v6> retrieval = nil;
    id<FxParameterSettingAPI_v5> setting = nil;
    id<FxKeyframeAPI_v3> keyframes = nil;
    UInt32 manifestID = 0;
    const UInt32 *chunkIDs = NULL;
    NSString *manifest = nil;
    NSString *oldManifest = nil;
    NSString *oldDisplayName = nil;
    NSArray<NSString *> *oldChunks = nil;
    NSArray<GFParameterSnapshot *> *parameterSnapshots = nil;
    BOOL mutationStarted = NO;
    BOOL committed = NO;
    [action startAction:sender];
    @try {
        retrieval =
            [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
        setting =
            [self.apiManager apiForProtocol:@protocol(FxParameterSettingAPI_v5)];
        keyframes = [self.apiManager apiForProtocol:@protocol(FxKeyframeAPI_v3)];
        if (retrieval == nil || setting == nil || keyframes == nil) {
            os_log_error(OS_LOG_DEFAULT,
                         "Gyroflow project payload rejected: action-scoped host parameter API unavailable");
        } else {
            NSString *currentManifestA = nil;
            NSString *currentManifestB = nil;
            BOOL prepared = [self readString:&currentManifestA
                                  parameterID:kGFProjectPayloadManifestA
                                     retrieval:retrieval] &&
                [self readString:&currentManifestB
                     parameterID:kGFProjectPayloadManifestB
                        retrieval:retrieval];
            NSDictionary *candidateA = prepared
                ? [self candidateForManifest:currentManifestA
                                    chunkIDs:kGFProjectPayloadChunksA
                                   retrieval:retrieval
                                        bank:@"A"
                                       error:NULL]
                : nil;
            NSDictionary *candidateB = prepared
                ? [self candidateForManifest:currentManifestB
                                    chunkIDs:kGFProjectPayloadChunksB
                                   retrieval:retrieval
                                        bank:@"B"
                                       error:NULL]
                : nil;
            NSError *selectionError = nil;
            if (prepared) {
                [self preferredCandidateA:candidateA
                               candidateB:candidateB
                                    error:&selectionError];
                prepared = selectionError == nil;
            }
            unsigned long long generationA =
                [candidateA[@"generation"] unsignedLongLongValue];
            unsigned long long generationB =
                [candidateB[@"generation"] unsignedLongLongValue];
            unsigned long long maximumGeneration = MAX(generationA, generationB);
            prepared = prepared && maximumGeneration != ULLONG_MAX;
            BOOL useBankA = candidateA == nil ||
                (candidateB != nil && generationA <= generationB);
            manifestID =
                useBankA ? kGFProjectPayloadManifestA : kGFProjectPayloadManifestB;
            chunkIDs = useBankA ? kGFProjectPayloadChunksA : kGFProjectPayloadChunksB;
            oldManifest = useBankA ? currentManifestA : currentManifestB;
            manifest = prepared
                ? [self manifestForPayloadData:payloadData
                                    generation:maximumGeneration + 1
                                    chunkCount:chunkCount]
                : nil;
            prepared = prepared && manifest != nil &&
                [self readString:&oldDisplayName
                     parameterID:kGFProjectDisplayName
                        retrieval:retrieval];

            NSMutableArray<NSString *> *snapshotChunks =
                [NSMutableArray arrayWithCapacity:kGFProjectPayloadChunksPerBank];
            for (NSUInteger index = 0;
                 index < kGFProjectPayloadChunksPerBank && prepared;
                 ++index) {
                NSString *chunk = nil;
                prepared = [self readString:&chunk
                                 parameterID:chunkIDs[index]
                                    retrieval:retrieval];
                if (prepared) {
                    [snapshotChunks addObject:chunk];
                }
            }
            oldChunks = snapshotChunks;
            parameterSnapshots = prepared
                ? [self parameterSnapshotsWithRetrieval:retrieval keyframes:keyframes]
                : nil;
            prepared = prepared && parameterSnapshots != nil;

            BOOL staged = prepared;
            for (NSUInteger index = 0;
                 index < kGFProjectPayloadChunksPerBank && staged;
                 ++index) {
                mutationStarted = YES;
                staged = [setting setStringParameterValue:chunks[index]
                                              toParameter:chunkIDs[index]];
            }
            if (staged) {
                mutationStarted = YES;
                staged = [self removeAllProjectKeyframes:keyframes];
            }
            if (staged) {
                staged = [self setStaticParameters:parameters setting:setting];
            }
            if (staged) {
                staged = [setting setStringParameterValue:displayName
                                               toParameter:kGFProjectDisplayName];
            }
            for (NSUInteger index = 0;
                 index < kGFProjectPayloadChunksPerBank && staged;
                 ++index) {
                NSString *stagedChunk = nil;
                staged = [self readString:&stagedChunk
                              parameterID:chunkIDs[index]
                                 retrieval:retrieval] &&
                    [stagedChunk isEqualToString:chunks[index]];
            }
            NSString *stagedDisplayName = nil;
            staged = staged &&
                [self readString:&stagedDisplayName
                     parameterID:kGFProjectDisplayName
                        retrieval:retrieval] &&
                [stagedDisplayName isEqualToString:displayName] &&
                [self staticParametersMatch:parameters retrieval:retrieval];
            if (staged) {
                staged = [setting setStringParameterValue:manifest
                                              toParameter:manifestID];
            }
            NSString *stagedManifest = nil;
            committed = staged &&
                [self readString:&stagedManifest
                     parameterID:manifestID
                        retrieval:retrieval] &&
                [stagedManifest isEqualToString:manifest];

            if (!committed && mutationStarted) {
                BOOL rolledBack = [self restoreSnapshots:parameterSnapshots
                                                chunkIDs:chunkIDs
                                               oldChunks:oldChunks
                                              manifestID:manifestID
                                             oldManifest:oldManifest
                                          oldDisplayName:oldDisplayName
                                                retrieval:retrieval
                                                  setting:setting
                                                keyframes:keyframes];
                if (!rolledBack) {
                    os_log_error(OS_LOG_DEFAULT,
                                 "Gyroflow project payload rollback could not be verified");
                }
            }
        }
    } @finally {
        [action endAction:sender];
    }
    if (!committed) {
        os_log_error(OS_LOG_DEFAULT,
                     "Gyroflow project payload rejected: exact action readback failed");
    }
    return committed;
}

@end
