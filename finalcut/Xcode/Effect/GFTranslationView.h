// SPDX-License-Identifier: GPL-3.0-or-later
#import <Cocoa/Cocoa.h>
#import "GyroflowFinalCut.h"

#import "GFTranslationSettings.h"

typedef GFTranslationInfo (^GFTranslationInfoHandler)(void);
typedef BOOL (^GFTranslationCommitHandler)(GFTranslationParameters parameters, NSView *sender);

@interface GFTranslationView : NSView
- (instancetype)initWithInfoHandler:(GFTranslationInfoHandler)infoHandler
                      commitHandler:(GFTranslationCommitHandler)commitHandler;
- (void)refresh;
@end
