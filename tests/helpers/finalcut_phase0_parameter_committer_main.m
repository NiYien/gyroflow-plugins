#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>
#import <objc/runtime.h>

#import "GFPhase0ParameterCommitter.h"


@interface GFTestActionAPI : NSObject
@property(nonatomic) BOOL active;
@property(nonatomic) NSUInteger startCount;
@property(nonatomic) NSUInteger endCount;
@property(nonatomic, strong) NSMutableArray<NSString *> *events;
@end


@implementation GFTestActionAPI

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.events = [NSMutableArray array];
    }
    return self;
}

- (void)startAction:(id)sender {
    (void)sender;
    self.startCount += 1;
    self.active = YES;
    [self.events addObject:@"start"];
}

- (void)endAction:(id)sender {
    (void)sender;
    [self.events addObject:@"end"];
    self.active = NO;
    self.endCount += 1;
}

@end


@interface GFTestParameterAPI : NSObject
@property(nonatomic, weak) GFTestActionAPI *action;
@property(nonatomic, strong) NSMutableDictionary<NSNumber *, NSString *> *values;
@property(nonatomic) NSUInteger outsideActionWrites;
@end


@implementation GFTestParameterAPI

- (instancetype)initWithAction:(GFTestActionAPI *)action {
    self = [super init];
    if (self != nil) {
        self.action = action;
        self.values = [NSMutableDictionary dictionary];
    }
    return self;
}

- (BOOL)getStringParameterValue:(NSString **)value fromParameter:(UInt32)parameterID {
    if (value != NULL) {
        *value = self.values[@(parameterID)] ?: @"";
    }
    return self.action.active;
}

- (BOOL)setStringParameterValue:(NSString *)value toParameter:(UInt32)parameterID {
    if (!self.action.active) {
        self.outsideActionWrites += 1;
        return NO;
    }
    self.values[@(parameterID)] = value;
    [self.action.events addObject:[NSString stringWithFormat:@"set-%u", parameterID]];
    return YES;
}

@end


@interface GFTestAPIManager : NSObject <PROAPIAccessing>
@property(nonatomic, strong) GFTestActionAPI *action;
@property(nonatomic, strong) GFTestParameterAPI *parameters;
@end


@implementation GFTestAPIManager

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        self.action = [[GFTestActionAPI alloc] init];
        self.parameters = [[GFTestParameterAPI alloc] initWithAction:self.action];
    }
    return self;
}

- (id)apiForProtocol:(Protocol *)apiProtocol {
    if (protocol_isEqual(apiProtocol, @protocol(FxCustomParameterActionAPI_v4))) {
        return self.action;
    }
    if (protocol_isEqual(apiProtocol, @protocol(FxParameterSettingAPI_v5)) ||
        protocol_isEqual(apiProtocol, @protocol(FxParameterRetrievalAPI_v6))) {
        return self.parameters;
    }
    return nil;
}

@end


int main(void) {
    @autoreleasepool {
        GFTestAPIManager *manager = [[GFTestAPIManager alloc] init];
        GFPhase0ParameterCommitter *committer =
            [[GFPhase0ParameterCommitter alloc] initWithAPIManager:manager
                                      instanceIdentityParameterID:1901
                                         projectPayloadParameterID:1902
                                               evidenceParameterID:1103];
        NSString *firstIdentity = nil;
        NSString *secondIdentity = nil;
        BOOL first = [committer commitProjectData:[@"first" dataUsingEncoding:NSUTF8StringEncoding]
                                            sender:manager
                                          identity:&firstIdentity];
        BOOL second = [committer commitProjectData:[@"second" dataUsingEncoding:NSUTF8StringEncoding]
                                             sender:manager
                                           identity:&secondIdentity];

        NSDictionary *result = @{
            @"first" : @(first),
            @"second" : @(second),
            @"startCount" : @(manager.action.startCount),
            @"endCount" : @(manager.action.endCount),
            @"outsideActionWrites" : @(manager.parameters.outsideActionWrites),
            @"identityLength" : @(firstIdentity.length),
            @"sameIdentity" : @([firstIdentity isEqualToString:secondIdentity]),
            @"projectPayload" : manager.parameters.values[@1902] ?: @"",
            @"evidence" : manager.parameters.values[@1103] ?: @"",
            @"events" : manager.action.events,
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return first && second ? 0 : 1;
    }
}
