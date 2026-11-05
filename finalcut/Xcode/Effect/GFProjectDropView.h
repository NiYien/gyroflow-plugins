#import <Cocoa/Cocoa.h>

@class GFProjectStore;
@class GFProjectImportCandidate;

typedef BOOL (^GFProjectImportCommitHandler)(GFProjectImportCandidate *candidate, NSView *sender);

@interface GFProjectDropView : NSView

- (instancetype)initWithProjectStore:(GFProjectStore *)projectStore
                       commitHandler:(GFProjectImportCommitHandler)commitHandler;
- (void)refreshStatus;

@end
