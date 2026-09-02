#import <Foundation/Foundation.h>


NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSString *const GFPhase0ProjectStoreErrorDomain;

typedef NS_ENUM(NSInteger, GFPhase0ProjectStoreErrorCode) {
    GFPhase0ProjectStoreErrorWrongExtension = 1,
    GFPhase0ProjectStoreErrorReadFailed = 2,
    GFPhase0ProjectStoreErrorInvalidJSON = 3,
    GFPhase0ProjectStoreErrorInvalidStructure = 4,
};

@interface GFPhase0ProjectStore : NSObject

@property(nonatomic, readonly, nullable) NSData *currentProjectData;
@property(nonatomic, readonly, nullable) NSNumber *currentProjectVersion;
@property(nonatomic, readonly) NSString *status;

- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error;
- (void)recordAuthorizationCancellation;

@end

NS_ASSUME_NONNULL_END
