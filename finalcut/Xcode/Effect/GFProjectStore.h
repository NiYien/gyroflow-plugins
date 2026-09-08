#import <Foundation/Foundation.h>
#import "GyroflowFinalCut.h"

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSString *const GFProjectStoreErrorDomain;

typedef NS_ENUM(NSInteger, GFProjectStoreErrorCode) {
    GFProjectStoreErrorWrongExtension = 1,
    GFProjectStoreErrorReadFailed = 2,
    GFProjectStoreErrorInvalidProject = 3,
    GFProjectStoreErrorInvalidPayload = 4,
    GFProjectStoreErrorCommitFailed = 5,
    GFProjectStoreErrorCommitPending = 6,
};

typedef NS_ENUM(NSInteger, GFProjectModeStatus) {
    GFProjectModeStatusNone = 0,
    GFProjectModeStatusPending = 1,
    GFProjectModeStatusDirect = 2,
    GFProjectModeStatusRouteD = 3,
    GFProjectModeStatusReprocessRequired = 4,
};

@interface GFProjectImportCandidate : NSObject

@property(nonatomic, readonly) NSData *projectData;
@property(nonatomic, readonly) NSString *projectPayload;
@property(nonatomic, readonly) NSString *projectDisplayName;
@property(nonatomic, readonly) GFRenderParameters parameters;

- (instancetype)initWithProjectData:(NSData *)projectData
                     projectPayload:(NSString *)projectPayload
                  projectDisplayName:(NSString *)projectDisplayName
                          parameters:(GFRenderParameters)parameters
    NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;

@end

typedef GFProjectImportCandidate * _Nullable (^GFProjectImportCandidateBuilder)(
    NSData *projectData,
    NSString *projectDisplayName,
    NSError **error
);

@interface GFProjectStore : NSObject

@property(nonatomic, readonly, nullable) NSData *currentProjectData;
@property(nonatomic, readonly, nullable) NSString *currentProjectPayload;
@property(nonatomic, readonly, nullable) GFProjectImportCandidate *currentProjectCandidate;
@property(nonatomic, readonly) NSString *currentProjectName;
@property(nonatomic, readonly) NSString *currentProjectFilename;
@property(nonatomic, readonly) NSString *status;
@property(nonatomic, readonly) GFProjectModeStatus modeStatus;
@property(nonatomic, readonly) NSUInteger currentProjectGeneration;

- (instancetype)initWithImportCandidateBuilder:
    (GFProjectImportCandidateBuilder)importCandidateBuilder;
- (nullable GFProjectImportCandidate *)prepareImportProjectURL:(NSURL *)url
                                                          error:(NSError **)error;
- (BOOL)commitPreparedImportCandidate:(GFProjectImportCandidate *)candidate
                            sourceURL:(NSURL *)sourceURL
                       commitCandidate:(nullable BOOL (^)(GFProjectImportCandidate *candidate))commitCandidate
                                 error:(NSError **)error;
- (void)recordImportFailureForURL:(NSURL *)url error:(NSError *)error;
- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error;
- (BOOL)importProjectURL:(NSURL *)url
         commitCandidate:(nullable BOOL (^)(GFProjectImportCandidate *candidate))commitCandidate
                   error:(NSError **)error;
- (BOOL)restoreProjectPayload:(NSString *)payload error:(NSError **)error;
- (BOOL)restorePersistedProjectPayload:(NSString *)projectPayload
                           displayName:(NSString *)displayName
                         timingPayload:(NSString *)timingPayload
                                 error:(NSError **)error;
- (BOOL)restoreValidatedRenderProjectPayloadIfEmpty:(NSString *)projectPayload
                                        displayName:(NSString *)displayName
                                      timingPayload:(NSString *)timingPayload;
- (NSUInteger)beginHostProjectReadback;
- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload;
- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload
                          readbackGeneration:(NSUInteger)readbackGeneration;
- (BOOL)failPendingHostReadbackWithMessage:(NSString *)message;
- (BOOL)expirePendingHostReadbackGeneration:(NSUInteger)generation;
- (void)recordDirectModeReady;
- (void)recordRouteDModeReady;
- (void)recordReprocessRequired;
- (void)recordAuthorizationCancellation;

@end

NS_ASSUME_NONNULL_END
