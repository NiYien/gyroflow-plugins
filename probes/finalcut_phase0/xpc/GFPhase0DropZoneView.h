#import <Cocoa/Cocoa.h>

@class GFPhase0ProjectStore;


typedef void (^GFPhase0ProjectCommitHandler)(NSData *projectData, NSView *sender);


@interface GFPhase0DropZoneView : NSView
- (instancetype)initWithProjectStore:(GFPhase0ProjectStore *)projectStore
                        commitHandler:(GFPhase0ProjectCommitHandler)commitHandler;
@end
