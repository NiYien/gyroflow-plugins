#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>


@interface GFPhase0ParameterCommitter : NSObject

- (instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager
        instanceIdentityParameterID:(UInt32)instanceIdentityParameterID
           projectPayloadParameterID:(UInt32)projectPayloadParameterID
                 evidenceParameterID:(UInt32)evidenceParameterID;

- (BOOL)commitProjectData:(NSData *)projectData
                   sender:(id)sender
                 identity:(NSString **)identity;

@end
