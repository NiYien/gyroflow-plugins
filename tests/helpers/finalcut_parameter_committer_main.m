#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>
#import <objc/runtime.h>
#include <string.h>

#import "GFParameterCommitter.h"
#import "GFParameterIDs.h"

@interface GFTestActionAPI : NSObject
@property(nonatomic) BOOL active;
@property(nonatomic) NSUInteger startCount;
@property(nonatomic) NSUInteger endCount;
@end

@implementation GFTestActionAPI
- (void)startAction:(id)sender {
    self.active = YES;
    self.startCount += 1;
}
- (void)endAction:(id)sender {
    self.active = NO;
    self.endCount += 1;
}
@end

@interface GFTestSettingAPI : NSObject
@property(nonatomic, weak) GFTestActionAPI *action;
@property(nonatomic) NSUInteger outsideActionWrites;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSString *> *values;
@property(nonatomic, strong) NSMutableArray<NSNumber *> *writeIDs;
@property(nonatomic, copy) NSSet<NSNumber *> *discardIDs;
@property(nonatomic) BOOL readsRequireAction;
@property(nonatomic) NSUInteger readCount;
@property(nonatomic) NSUInteger insideActionReads;
@end

@implementation GFTestSettingAPI
- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.values = [NSMutableDictionary dictionary];
        self.writeIDs = [NSMutableArray array];
        self.discardIDs = [NSSet set];
    }
    return self;
}

- (BOOL)setStringParameterValue:(NSString *)value toParameter:(UInt32)parameterID {
    if (!self.action.active) {
        self.outsideActionWrites += 1;
        return NO;
    }
    NSNumber *key = @(parameterID);
    [self.writeIDs addObject:key];
    if (![self.discardIDs containsObject:key]) {
        self.values[key] = value ?: @"";
    }
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
    *value = self.values[@(parameterID)] ?: @"";
    return YES;
}
@end

@interface GFTestAPIManager : NSObject <PROAPIAccessing>
@property(nonatomic, strong) GFTestActionAPI *action;
@property(nonatomic, strong) GFTestSettingAPI *setting;
@property(nonatomic) BOOL parameterAPIsRequireAction;
@end

@implementation GFTestAPIManager
- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.action = [[GFTestActionAPI alloc] init];
        self.setting = [[GFTestSettingAPI alloc] init];
        self.setting.action = self.action;
    }
    return self;
}
- (id)apiForProtocol:(Protocol *)apiProtocol {
    if (protocol_isEqual(apiProtocol, @protocol(FxCustomParameterActionAPI_v4))) {
        return self.action;
    }
    if (protocol_isEqual(apiProtocol, @protocol(FxParameterSettingAPI_v5))) {
        if (self.parameterAPIsRequireAction && !self.action.active) {
            return nil;
        }
        return self.setting;
    }
    if (protocol_isEqual(apiProtocol, @protocol(FxParameterRetrievalAPI_v6))) {
        if (self.parameterAPIsRequireAction && !self.action.active) {
            return nil;
        }
        return self.setting;
    }
    return nil;
}
@end

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
    if (json == nil) {
        return @{};
    }
    id value = [NSJSONSerialization JSONObjectWithData:json options:0 error:NULL];
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
        if ([values[@(chunkIDs[index])] length] > 0) {
            count += 1;
        }
    }
    return count;
}

static NSUInteger GFWriteCount(NSArray<NSNumber *> *writeIDs, UInt32 parameterID) {
    NSUInteger count = 0;
    for (NSNumber *writeID in writeIDs) {
        if (writeID.unsignedIntValue == parameterID) {
            count += 1;
        }
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

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        GFTestAPIManager *manager = [[GFTestAPIManager alloc] init];
        manager.setting.values[@(kGFProjectPayload)] = @"previous-payload";
        GFParameterCommitter *committer =
            [[GFParameterCommitter alloc] initWithAPIManager:manager];

        if (argc > 1 && strcmp(argv[1], "--cycle") == 0) {
            NSString *payloadA = GFRepeatedPayload(@"QUFB", 300000);
            NSString *payloadB = GFRepeatedPayload(@"QkJC", 300000);
            NSString *payloadC = GFRepeatedPayload(@"Q0ND", 300000);
            BOOL first = [committer commitProjectPayload:payloadA sender:manager];
            BOOL second = [committer commitProjectPayload:payloadB sender:manager];
            BOOL third = [committer commitProjectPayload:payloadC sender:manager];
            NSError *latestError = nil;
            NSString *latest = [committer persistedProjectPayloadWithError:&latestError];
            manager.setting.values[@(kGFProjectPayloadChunksA[0])] = @"corrupt";
            GFParameterCommitter *restarted =
                [[GFParameterCommitter alloc] initWithAPIManager:manager];
            NSError *fallbackError = nil;
            NSString *fallback =
                [restarted persistedProjectPayloadWithError:&fallbackError];
            NSDictionary *manifestA = GFDecodeManifest(
                manager.setting.values[@(kGFProjectPayloadManifestA)]);
            NSDictionary *manifestB = GFDecodeManifest(
                manager.setting.values[@(kGFProjectPayloadManifestB)]);
            GFPrintJSON(@{
                @"manifestWriteIDs" : GFManifestWriteIDs(manager.setting.writeIDs),
                @"generationA" : manifestA[@"generation"] ?: @0,
                @"generationB" : manifestB[@"generation"] ?: @0,
                @"latestRecovered" : @(
                    latestError == nil && [latest isEqualToString:payloadC]),
                @"fallbackRecovered" : @(
                    fallbackError == nil && [fallback isEqualToString:payloadB]),
                @"legacyWriteCount" : @(
                    GFWriteCount(manager.setting.writeIDs, kGFProjectPayload)),
            });
            return first && second && third && latestError == nil &&
                fallbackError == nil ? 0 : 1;
        }

        if (argc > 1 && strcmp(argv[1], "--conflict") == 0) {
            NSString *payloadA = GFRepeatedPayload(@"QUFB", 300000);
            NSString *payloadB = GFRepeatedPayload(@"QkJC", 300000);
            BOOL first = [committer commitProjectPayload:payloadA sender:manager];
            BOOL second = [committer commitProjectPayload:payloadB sender:manager];
            NSMutableDictionary *manifestA = [GFDecodeManifest(
                manager.setting.values[@(kGFProjectPayloadManifestA)]) mutableCopy];
            manifestA[@"generation"] = @2;
            manager.setting.values[@(kGFProjectPayloadManifestA)] =
                GFEncodeManifest(manifestA);
            GFParameterCommitter *restarted =
                [[GFParameterCommitter alloc] initWithAPIManager:manager];
            NSError *error = nil;
            NSString *recovered = [restarted persistedProjectPayloadWithError:&error];
            GFPrintJSON(@{
                @"recovered" : @(recovered != nil),
                @"error" : error.localizedDescription ?: @"",
            });
            return first && second && recovered == nil && error != nil ? 0 : 1;
        }

        if (argc > 1 && strcmp(argv[1], "--capacity") == 0) {
            NSString *maximum = GFRepeatedPayload(@"QUFB", kGFProjectPayloadMaximumBytes);
            BOOL maximumCommitted =
                [committer commitProjectPayload:maximum sender:manager];
            NSUInteger startCountAfterMaximum = manager.action.startCount;
            NSString *oversized = [maximum stringByAppendingString:@"A"];
            BOOL oversizedCommitted =
                [committer commitProjectPayload:oversized sender:manager];
            GFPrintJSON(@{
                @"maximumCommitted" : @(maximumCommitted),
                @"oversizedCommitted" : @(oversizedCommitted),
                @"startCountAfterMaximum" : @(startCountAfterMaximum),
                @"startCountAfterOversized" : @(manager.action.startCount),
                @"maximumUsedChunks" : @(
                    GFNonEmptyChunkCount(manager.setting.values,
                                         kGFProjectPayloadChunksA)),
            });
            return maximumCommitted && !oversizedCommitted ? 0 : 1;
        }

        NSString *payload = @"dmVyc2lvbmVkLWJhc2U2NC1lbnZlbG9wZQ==";
        if (argc > 1 && strcmp(argv[1], "--discard-write") == 0) {
            manager.setting.discardIDs = [NSSet setWithObject:@(kGFProjectPayloadManifestA)];
        }
        if (argc > 1 && strcmp(argv[1], "--action-gated-apis") == 0) {
            manager.parameterAPIsRequireAction = YES;
            manager.setting.readsRequireAction = YES;
        }
        BOOL committed = [committer commitProjectPayload:payload sender:manager];
        NSError *recoveryError = nil;
        NSString *recovered =
            [committer persistedProjectPayloadWithError:&recoveryError];
        NSDictionary *manifestA = GFDecodeManifest(
            manager.setting.values[@(kGFProjectPayloadManifestA)]);
        NSDictionary *result = @{
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
            @"writeCount" : @(manager.setting.writeIDs.count),
            @"legacyWriteCount" : @(
                GFWriteCount(manager.setting.writeIDs, kGFProjectPayload)),
            @"nonEmptyChunksA" : @(
                GFNonEmptyChunkCount(manager.setting.values,
                                     kGFProjectPayloadChunksA)),
            @"manifestA" : manifestA,
        };
        GFPrintJSON(result);
        return committed ? 0 : 1;
    }
}
