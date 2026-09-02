#import "GFRenderState.h"

static const NSInteger kGFRenderStateSchema = 1;

@interface GFRenderState ()
@property(nonatomic, readwrite) NSString *projectPayload;
@property(nonatomic, readwrite) NSString *timingPayload;
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
    self = [super init];
    if (self != nil) {
        self.projectPayload = [projectPayload copy];
        self.timingPayload = [timingPayload copy];
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
    if (schema != kGFRenderStateSchema ||
        projectPayload == nil ||
        timingPayload == nil) {
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
    return [self initWithProjectPayload:projectPayload
                         timingPayload:timingPayload
                            parameters:parameters
                          effectBounds:effectBounds
                           inputBounds:inputBounds];
}

- (void)encodeWithCoder:(NSCoder *)coder {
    [coder encodeInteger:kGFRenderStateSchema forKey:@"schema"];
    [coder encodeObject:self.projectPayload forKey:@"projectPayload"];
    [coder encodeObject:self.timingPayload forKey:@"timingPayload"];
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
