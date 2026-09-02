#import "GFParameterCommitter.h"

#import "GFParameterIDs.h"
#import <CommonCrypto/CommonDigest.h>
#import <os/log.h>

static const NSUInteger kGFProjectPayloadManifestVersion = 1;
static NSString *const GFProjectPayloadErrorDomain =
    @"com.niyien.gyroflow.finalcut.project-payload";

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
            return legacy;
        }
        if (bankAError != nil || bankBError != nil) {
            if (error != NULL) {
                *error = bankAError ?: bankBError;
            }
            return nil;
        }
        return @"";
    }

    [self.cacheLock lock];
    self.cachedManifestA = manifestA;
    self.cachedManifestB = manifestB;
    self.cachedPayload = payload;
    [self.cacheLock unlock];
    return payload;
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

- (BOOL)commitProjectPayload:(NSString *)projectPayload sender:(id)sender {
    if (projectPayload.length == 0 || sender == nil) {
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
    [self.cacheLock unlock];

    id<FxParameterRetrievalAPI_v6> retrieval = nil;
    UInt32 manifestID = 0;
    const UInt32 *chunkIDs = NULL;
    NSString *manifest = nil;
    BOOL committed = NO;
    [action startAction:sender];
    @try {
        retrieval =
            [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
        id<FxParameterSettingAPI_v5> setting =
            [self.apiManager apiForProtocol:@protocol(FxParameterSettingAPI_v5)];
        if (retrieval == nil || setting == nil) {
            os_log_error(OS_LOG_DEFAULT,
                         "Gyroflow project payload rejected: action-scoped host parameter API unavailable");
            return NO;
        }

        NSString *currentManifestA = nil;
        NSString *currentManifestB = nil;
        if (![self readString:&currentManifestA
                  parameterID:kGFProjectPayloadManifestA
                     retrieval:retrieval] ||
            ![self readString:&currentManifestB
                  parameterID:kGFProjectPayloadManifestB
                     retrieval:retrieval]) {
            os_log_error(OS_LOG_DEFAULT,
                         "Gyroflow project payload rejected: current manifests unreadable");
            return NO;
        }
        NSDictionary *candidateA =
            [self candidateForManifest:currentManifestA
                              chunkIDs:kGFProjectPayloadChunksA
                             retrieval:retrieval
                                  bank:@"A"
                                 error:NULL];
        NSDictionary *candidateB =
            [self candidateForManifest:currentManifestB
                              chunkIDs:kGFProjectPayloadChunksB
                             retrieval:retrieval
                                  bank:@"B"
                                 error:NULL];
        NSError *selectionError = nil;
        [self preferredCandidateA:candidateA
                       candidateB:candidateB
                            error:&selectionError];
        if (selectionError != nil) {
            os_log_error(OS_LOG_DEFAULT,
                         "Gyroflow project payload rejected: %{public}@",
                         selectionError.localizedDescription);
            return NO;
        }

        unsigned long long generationA =
            [candidateA[@"generation"] unsignedLongLongValue];
        unsigned long long generationB =
            [candidateB[@"generation"] unsignedLongLongValue];
        unsigned long long maximumGeneration = MAX(generationA, generationB);
        if (maximumGeneration == ULLONG_MAX) {
            os_log_error(OS_LOG_DEFAULT,
                         "Gyroflow project payload rejected: generation exhausted");
            return NO;
        }
        BOOL useBankA = candidateA == nil ||
            (candidateB != nil && generationA <= generationB);
        manifestID =
            useBankA ? kGFProjectPayloadManifestA : kGFProjectPayloadManifestB;
        chunkIDs = useBankA ? kGFProjectPayloadChunksA : kGFProjectPayloadChunksB;
        manifest = [self manifestForPayloadData:payloadData
                                     generation:maximumGeneration + 1
                                     chunkCount:chunkCount];
        if (manifest == nil) {
            return NO;
        }

        committed = YES;
        for (NSUInteger index = 0;
             index < kGFProjectPayloadChunksPerBank && committed;
             ++index) {
            committed = [setting setStringParameterValue:chunks[index]
                                              toParameter:chunkIDs[index]];
        }
        if (committed) {
            committed = [setting setStringParameterValue:manifest
                                              toParameter:manifestID];
        }
        if (committed) {
            for (NSUInteger index = 0;
                 index < kGFProjectPayloadChunksPerBank && committed;
                 ++index) {
                NSString *stagedChunk = nil;
                committed = [self readString:&stagedChunk
                                  parameterID:chunkIDs[index]
                                     retrieval:retrieval] &&
                    [stagedChunk isEqualToString:chunks[index]];
            }
            NSString *stagedManifest = nil;
            committed = committed &&
                [self readString:&stagedManifest
                      parameterID:manifestID
                         retrieval:retrieval] &&
                [stagedManifest isEqualToString:manifest];
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
