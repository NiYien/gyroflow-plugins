#import <Cocoa/Cocoa.h>

@class GFProjectStore;

typedef BOOL (^GFProjectPayloadCommitHandler)(NSString *projectPayload, NSView *sender);

@interface GFProjectDropView : NSView

- (instancetype)initWithProjectStore:(GFProjectStore *)projectStore
                       commitHandler:(GFProjectPayloadCommitHandler)commitHandler;
- (void)refreshStatus;

@end
