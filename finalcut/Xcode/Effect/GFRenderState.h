#import "GyroflowFinalCut.h"
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

typedef NS_ENUM(NSInteger, GFRenderMode) {
    GFRenderModeEmpty = 0,
    GFRenderModeDirect = 1,
    GFRenderModeRouteD = 2,
    GFRenderModeReprocessRequired = 3,
};

@interface GFRenderState : NSObject <NSSecureCoding, NSCopying>

@property(nonatomic, readonly) NSInteger schemaVersion;
@property(nonatomic, readonly) NSString *projectPayload;
@property(nonatomic, readonly) NSString *projectDisplayName;
@property(nonatomic, readonly) NSString *projectContentHash;
@property(nonatomic, readonly) NSString *timingPayload;
@property(nonatomic, readonly) GFRenderMode mode;
@property(nonatomic, readonly) GFRenderParameters parameters;
@property(nonatomic, readonly) GFTimeRange effectBounds;
@property(nonatomic, readonly) GFTimeRange inputBounds;

- (instancetype)initWithProjectPayload:(NSString *)projectPayload
                         timingPayload:(NSString *)timingPayload
                            parameters:(GFRenderParameters)parameters
                          effectBounds:(GFTimeRange)effectBounds
                           inputBounds:(GFTimeRange)inputBounds;

- (instancetype)initWithProjectPayload:(NSString *)projectPayload
                     projectDisplayName:(NSString *)projectDisplayName
                     projectContentHash:(NSString *)projectContentHash
                         timingPayload:(NSString *)timingPayload
                                  mode:(GFRenderMode)mode
                            parameters:(GFRenderParameters)parameters
                          effectBounds:(GFTimeRange)effectBounds
                           inputBounds:(GFTimeRange)inputBounds NS_DESIGNATED_INITIALIZER;
- (instancetype)init NS_UNAVAILABLE;

@end

NS_ASSUME_NONNULL_END
