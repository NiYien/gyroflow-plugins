#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>

NS_ASSUME_NONNULL_BEGIN

@interface GFParameterCommitter : NSObject

- (instancetype)initWithAPIManager:(id<PROAPIAccessing>)apiManager
    NS_DESIGNATED_INITIALIZER;

- (instancetype)init NS_UNAVAILABLE;

- (BOOL)commitProjectPayload:(NSString *)projectPayload sender:(id)sender;

- (nullable NSString *)persistedProjectPayloadWithError:
    (NSError * _Nullable * _Nullable)error;

@end

NS_ASSUME_NONNULL_END
