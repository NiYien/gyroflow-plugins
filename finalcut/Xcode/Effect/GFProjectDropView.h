#import <Cocoa/Cocoa.h>

@class GFProjectStore;
@class GFProjectImportCandidate;

typedef BOOL (^GFProjectImportCommitHandler)(GFProjectImportCandidate *candidate, NSView *sender);

@interface GFProjectDropView : NSView

- (instancetype)initWithProjectStore:(GFProjectStore *)projectStore
                       commitHandler:(GFProjectImportCommitHandler)commitHandler;
@property(nonatomic, copy) void (^samplingHandler)(NSView *sender);
- (void)refreshStatus;

@end
