#import "GFPhase0ParameterCommitter.h"

#import <os/log.h>


@interface GFPhase0ParameterCommitter ()
@property(nonatomic, strong) id<PROAPIAccessing> apiManager;
@property(nonatomic) UInt32 instanceIdentityParameterID;
@property(nonatomic) UInt32 projectPayloadParameterID;
@property(nonatomic) UInt32 evidenceParameterID;
@end


@implementation GFPhase0ParameterCommitter

- (instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager
        instanceIdentityParameterID:(UInt32)instanceIdentityParameterID
           projectPayloadParameterID:(UInt32)projectPayloadParameterID
                 evidenceParameterID:(UInt32)evidenceParameterID {
    self = [super init];
    if (self != nil) {
        self.apiManager = apiManager;
        self.instanceIdentityParameterID = instanceIdentityParameterID;
        self.projectPayloadParameterID = projectPayloadParameterID;
        self.evidenceParameterID = evidenceParameterID;
    }
    return self;
}

- (BOOL)commitProjectData:(NSData *)projectData
                   sender:(id)sender
                 identity:(NSString **)identity {
    if (projectData == nil || sender == nil) {
        return NO;
    }

    id<FxCustomParameterActionAPI_v4> action =
        [self.apiManager apiForProtocol:@protocol(FxCustomParameterActionAPI_v4)];
    if (action == nil) {
        os_log_error(OS_LOG_DEFAULT,
                     "phase0 project payload rejected reason=action_api_unavailable");
        return NO;
    }

    __block BOOL committed = NO;
    __block NSString *resolvedIdentity = @"";
    [action startAction:sender];
    @try {
        id<FxParameterRetrievalAPI_v6> retrieval =
            [self.apiManager apiForProtocol:@protocol(FxParameterRetrievalAPI_v6)];
        id<FxParameterSettingAPI_v5> setting =
            [self.apiManager apiForProtocol:@protocol(FxParameterSettingAPI_v5)];
        if (retrieval == nil || setting == nil) {
            os_log_error(OS_LOG_DEFAULT,
                         "phase0 project payload rejected reason=parameter_api_unavailable retrieval=%{public}d setting=%{public}d",
                         retrieval != nil,
                         setting != nil);
        } else {
            NSString *storedIdentity = nil;
            BOOL identityRead =
                [retrieval getStringParameterValue:&storedIdentity
                                      fromParameter:self.instanceIdentityParameterID];
            if (!identityRead) {
                os_log_error(OS_LOG_DEFAULT,
                             "phase0 project payload rejected reason=identity_read_failed");
            } else {
                resolvedIdentity = storedIdentity ?: @"";
                BOOL identitySet = YES;
                if (resolvedIdentity.length == 0) {
                    resolvedIdentity = NSUUID.UUID.UUIDString;
                    identitySet =
                        [setting setStringParameterValue:resolvedIdentity
                                            toParameter:self.instanceIdentityParameterID];
                }

                NSString *payload = [projectData base64EncodedStringWithOptions:0];
                BOOL payloadSet =
                    [setting setStringParameterValue:payload
                                        toParameter:self.projectPayloadParameterID];
                NSString *identitySuffix =
                    resolvedIdentity.length > 8
                        ? [resolvedIdentity substringFromIndex:resolvedIdentity.length - 8]
                        : resolvedIdentity;
                NSString *evidence =
                    [NSString stringWithFormat:@"instance=%@ payload_base64=%lu",
                                               identitySuffix,
                                               (unsigned long)payload.length];
                BOOL evidenceSet =
                    [setting setStringParameterValue:evidence
                                        toParameter:self.evidenceParameterID];
                committed = identitySet && payloadSet && evidenceSet;
                os_log_info(OS_LOG_DEFAULT,
                            "phase0 project payload action committed=%{public}d instance=%{public}@ project_bytes=%{public}lu base64_bytes=%{public}lu identity_set=%{public}d payload_set=%{public}d evidence_set=%{public}d",
                            committed,
                            resolvedIdentity,
                            (unsigned long)projectData.length,
                            (unsigned long)payload.length,
                            identitySet,
                            payloadSet,
                            evidenceSet);
            }
        }
    } @finally {
        [action endAction:sender];
    }

    if (identity != NULL) {
        *identity = resolvedIdentity;
    }
    return committed;
}

@end
