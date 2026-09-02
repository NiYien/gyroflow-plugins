#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSString *const GFProjectStoreErrorDomain;

typedef NS_ENUM(NSInteger, GFProjectStoreErrorCode) {
    GFProjectStoreErrorWrongExtension = 1,
    GFProjectStoreErrorReadFailed = 2,
    GFProjectStoreErrorInvalidProject = 3,
    GFProjectStoreErrorInvalidPayload = 4,
    GFProjectStoreErrorCommitFailed = 5,
};

typedef BOOL (^GFProjectPayloadBuilder)(
    NSData *projectData,
    NSString * _Nullable * _Nullable payload,
    NSError **error
);

@interface GFProjectStore : NSObject

@property(nonatomic, readonly, nullable) NSData *currentProjectData;
@property(nonatomic, readonly, nullable) NSString *currentProjectPayload;
@property(nonatomic, readonly) NSString *status;

- (instancetype)initWithPayloadBuilder:(GFProjectPayloadBuilder)payloadBuilder;
- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error;
- (BOOL)importProjectURL:(NSURL *)url
           commitPayload:(nullable BOOL (^)(NSString *projectPayload))commitPayload
                   error:(NSError **)error;
- (BOOL)restoreProjectPayload:(NSString *)payload error:(NSError **)error;
- (BOOL)restorePersistedProjectPayload:(NSString *)projectPayload
                         timingPayload:(NSString *)timingPayload
                                 error:(NSError **)error;
- (BOOL)restoreValidatedRenderProjectPayloadIfEmpty:(NSString *)projectPayload
                                      timingPayload:(NSString *)timingPayload;
- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload;
- (void)recordDirectModeReady;
- (void)recordReprocessRequired;
- (void)recordAuthorizationCancellation;

@end

NS_ASSUME_NONNULL_END
