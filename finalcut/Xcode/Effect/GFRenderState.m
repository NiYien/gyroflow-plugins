#import "GFRenderState.h"
#import <CommonCrypto/CommonDigest.h>

static const NSInteger kGFRenderStateSchema = 2;

static NSString *GFRenderStatePayloadHash(NSString *payload) {
    NSData *data = [payload dataUsingEncoding:NSASCIIStringEncoding] ?: [NSData data];
    unsigned char digest[CC_SHA256_DIGEST_LENGTH];
    CC_SHA256(data.bytes, (CC_LONG)data.length, digest);
    NSMutableString *result = [NSMutableString stringWithCapacity:CC_SHA256_DIGEST_LENGTH * 2];
    for (NSUInteger index = 0; index < CC_SHA256_DIGEST_LENGTH; ++index) {
        [result appendFormat:@"%02x", digest[index]];
    }
    return result;
}

@interface GFRenderState ()
@property(nonatomic, readwrite) NSInteger schemaVersion;
@property(nonatomic, readwrite) NSString *projectPayload;
@property(nonatomic, readwrite) NSString *projectDisplayName;
@property(nonatomic, readwrite) NSString *projectContentHash;
@property(nonatomic, readwrite) NSString *timingPayload;
@property(nonatomic, readwrite) GFRenderMode mode;
@property(nonatomic, readwrite) GFRenderParameters parameters;
@property(nonatomic, readwrite) GFTimeRange effectBounds;
@property(nonatomic, readwrite) GFTimeRange inputBounds;
@end

@implementation GFRenderState

+ (BOOL)supportsSecureCoding {
    return YES;
}

- (instancetype)initWithProjectPayload:(NSString *)projectPayload
                         timingPayload:(NSString *)timingPayload
                            parameters:(GFRenderParameters)parameters
                          effectBounds:(GFTimeRange)effectBounds
                           inputBounds:(GFTimeRange)inputBounds {
    GFRenderMode mode = projectPayload.length == 0
        ? GFRenderModeEmpty
        : (timingPayload.length == 0 ? GFRenderModeDirect : GFRenderModeRouteD);
    return [self initWithProjectPayload:projectPayload
                    projectDisplayName:@""
                    projectContentHash:GFRenderStatePayloadHash(projectPayload)
                        timingPayload:timingPayload
                                 mode:mode
                           parameters:parameters
                         effectBounds:effectBounds
                          inputBounds:inputBounds];
}

- (instancetype)initWithProjectPayload:(NSString *)projectPayload
                     projectDisplayName:(NSString *)projectDisplayName
                     projectContentHash:(NSString *)projectContentHash
                         timingPayload:(NSString *)timingPayload
                                  mode:(GFRenderMode)mode
                            parameters:(GFRenderParameters)parameters
                          effectBounds:(GFTimeRange)effectBounds
                           inputBounds:(GFTimeRange)inputBounds {
    self = [super init];
    if (self != nil) {
        self.schemaVersion = kGFRenderStateSchema;
        self.projectPayload = [projectPayload copy];
        self.projectDisplayName = [projectDisplayName copy];
        self.projectContentHash = projectContentHash.length > 0
            ? [projectContentHash copy]
            : GFRenderStatePayloadHash(projectPayload);
        self.timingPayload = [timingPayload copy];
        self.mode = mode;
        self.parameters = parameters;
        self.effectBounds = effectBounds;
        self.inputBounds = inputBounds;
    }
    return self;
}

- (instancetype)initWithCoder:(NSCoder *)coder {
    NSInteger schema = [coder decodeIntegerForKey:@"schema"];
    NSString *projectPayload = [coder decodeObjectOfClass:[NSString class]
                                                   forKey:@"projectPayload"];
    NSString *timingPayload = [coder decodeObjectOfClass:[NSString class]
                                                  forKey:@"timingPayload"];
    if ((schema != 1 && schema != kGFRenderStateSchema) ||
        projectPayload == nil ||
        timingPayload == nil) {
        return nil;
    }
    NSString *projectDisplayName = schema >= 2
        ? [coder decodeObjectOfClass:[NSString class] forKey:@"projectDisplayName"]
        : @"";
    NSString *projectContentHash = schema >= 2
        ? [coder decodeObjectOfClass:[NSString class] forKey:@"projectContentHash"]
        : GFRenderStatePayloadHash(projectPayload);
    GFRenderMode mode = schema >= 2
        ? (GFRenderMode)[coder decodeIntegerForKey:@"mode"]
        : (projectPayload.length == 0
            ? GFRenderModeEmpty
            : (timingPayload.length == 0 ? GFRenderModeDirect : GFRenderModeRouteD));
    if (projectDisplayName == nil || projectContentHash.length == 0 ||
        mode < GFRenderModeEmpty || mode > GFRenderModeReprocessRequired) {
        return nil;
    }
    GFRenderParameters parameters = {
        .fov = [coder decodeDoubleForKey:@"fov"],
        .smoothness = [coder decodeDoubleForKey:@"smoothness"],
        .lens_correction = [coder decodeDoubleForKey:@"lensCorrection"],
        .horizon_lock_amount = [coder decodeDoubleForKey:@"horizonLockAmount"],
        .horizon_lock_roll = [coder decodeDoubleForKey:@"horizonLockRoll"],
        .zoom_mode = (int32_t)[coder decodeInt32ForKey:@"zoomMode"],
        .overview = [coder decodeBoolForKey:@"overview"] ? 1 : 0,
        .reserved = {0, 0, 0},
    };
    GFTimeRange effectBounds = {
        .start = {
            .numerator = [coder decodeInt64ForKey:@"effectStartNumerator"],
            .denominator = [coder decodeInt64ForKey:@"effectStartDenominator"],
        },
        .duration = {
            .numerator = [coder decodeInt64ForKey:@"effectDurationNumerator"],
            .denominator = [coder decodeInt64ForKey:@"effectDurationDenominator"],
        },
    };
    GFTimeRange inputBounds = {
        .start = {
            .numerator = [coder decodeInt64ForKey:@"inputStartNumerator"],
            .denominator = [coder decodeInt64ForKey:@"inputStartDenominator"],
        },
        .duration = {
            .numerator = [coder decodeInt64ForKey:@"inputDurationNumerator"],
            .denominator = [coder decodeInt64ForKey:@"inputDurationDenominator"],
        },
    };
    GFRenderState *state = [self initWithProjectPayload:projectPayload
                                    projectDisplayName:projectDisplayName
                                    projectContentHash:projectContentHash
                                        timingPayload:timingPayload
                                                 mode:mode
                                           parameters:parameters
                                         effectBounds:effectBounds
                                          inputBounds:inputBounds];
    state.schemaVersion = schema;
    return state;
}

- (void)encodeWithCoder:(NSCoder *)coder {
    [coder encodeInteger:kGFRenderStateSchema forKey:@"schema"];
    [coder encodeObject:self.projectPayload forKey:@"projectPayload"];
    [coder encodeObject:self.projectDisplayName forKey:@"projectDisplayName"];
    [coder encodeObject:self.projectContentHash forKey:@"projectContentHash"];
    [coder encodeObject:self.timingPayload forKey:@"timingPayload"];
    [coder encodeInteger:self.mode forKey:@"mode"];
    [coder encodeDouble:self.parameters.fov forKey:@"fov"];
    [coder encodeDouble:self.parameters.smoothness forKey:@"smoothness"];
    [coder encodeDouble:self.parameters.lens_correction forKey:@"lensCorrection"];
    [coder encodeDouble:self.parameters.horizon_lock_amount forKey:@"horizonLockAmount"];
    [coder encodeDouble:self.parameters.horizon_lock_roll forKey:@"horizonLockRoll"];
    [coder encodeInt32:self.parameters.zoom_mode forKey:@"zoomMode"];
    [coder encodeBool:self.parameters.overview != 0 forKey:@"overview"];
    [coder encodeInt64:self.effectBounds.start.numerator forKey:@"effectStartNumerator"];
    [coder encodeInt64:self.effectBounds.start.denominator forKey:@"effectStartDenominator"];
    [coder encodeInt64:self.effectBounds.duration.numerator forKey:@"effectDurationNumerator"];
    [coder encodeInt64:self.effectBounds.duration.denominator forKey:@"effectDurationDenominator"];
    [coder encodeInt64:self.inputBounds.start.numerator forKey:@"inputStartNumerator"];
    [coder encodeInt64:self.inputBounds.start.denominator forKey:@"inputStartDenominator"];
    [coder encodeInt64:self.inputBounds.duration.numerator forKey:@"inputDurationNumerator"];
    [coder encodeInt64:self.inputBounds.duration.denominator forKey:@"inputDurationDenominator"];
}

- (id)copyWithZone:(NSZone *)zone {
    return self;
}

@end
