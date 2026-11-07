// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFTranslationView.h"
#import "GFLocalization.h"

@interface GFTranslationView ()
@property(nonatomic, copy) GFTranslationInfoHandler infoHandler;
@property(nonatomic, copy) GFTranslationCommitHandler commitHandler;
@property(nonatomic) GFTranslationInfo info;
@property(nonatomic, strong) NSButton *enabledButton;
@end

@implementation GFTranslationView

- (instancetype)initWithInfoHandler:(GFTranslationInfoHandler)infoHandler
                      commitHandler:(GFTranslationCommitHandler)commitHandler {
    self = [super initWithFrame:NSMakeRect(0, 0, 280, 30)];
    if (self == nil) { return nil; }
    self.infoHandler = infoHandler;
    self.commitHandler = commitHandler;
    self.enabledButton = [NSButton checkboxWithTitle:
        GFLocalized(@"effect.translation.enabled", @"Translation stabilization")
        target:self action:@selector(parametersChanged:)];
    self.enabledButton.frame = NSMakeRect(8, 4, 264, 22);
    self.enabledButton.autoresizingMask = NSViewWidthSizable;
    [self addSubview:self.enabledButton];
    [self refresh];
    return self;
}

- (void)refresh {
    self.info = self.infoHandler != nil ? self.infoHandler() : (GFTranslationInfo){0};
    self.enabledButton.state = self.info.parameters.enabled ? NSControlStateValueOn : NSControlStateValueOff;
    self.enabledButton.enabled = self.info.available != 0;
    self.enabledButton.toolTip = GFLocalized(@"effect.translation.help", @"Use the saved translation analysis. Disabling keeps the analysis.");
}

- (void)parametersChanged:(id)sender {
    (void)sender;
    if (!self.info.available) { return; }
    // Preserve the saved advanced settings when toggling stabilization.
    GFTranslationParameters parameters = self.info.parameters;
    parameters.enabled = self.enabledButton.state == NSControlStateValueOn;
    parameters.initialized = 1;
    BOOL committed = self.commitHandler != nil && self.commitHandler(parameters, self);
    [self refresh];
    if (!committed) {
        self.enabledButton.toolTip = GFLocalized(@"effect.translation.save_failed", @"Unable to save translation settings");
    }
}
@end
