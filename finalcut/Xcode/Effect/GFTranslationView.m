// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFTranslationView.h"
#import "GFLocalization.h"
#import <math.h>

@interface GFTranslationView ()
@property(nonatomic, copy) GFTranslationInfoHandler infoHandler;
@property(nonatomic, copy) GFTranslationCommitHandler commitHandler;
@property(nonatomic) GFTranslationInfo info;
@property(nonatomic, strong) NSButton *enabledButton;
@property(nonatomic, strong) NSButton *automaticButton;
@property(nonatomic, strong) NSButton *axisButton;
@property(nonatomic, strong) NSSlider *referenceSlider;
@property(nonatomic, strong) NSSlider *smoothnessSlider;
@property(nonatomic, strong) NSTextField *referenceLabel;
@property(nonatomic, strong) NSTextField *smoothnessLabel;
@property(nonatomic, strong) NSTextField *statusLabel;
@end

@implementation GFTranslationView

- (NSButton *)checkbox:(NSString *)title y:(CGFloat)y {
    NSButton *button = [NSButton checkboxWithTitle:title target:self action:@selector(parametersChanged:)];
    button.frame = NSMakeRect(8, y, 264, 22);
    button.autoresizingMask = NSViewWidthSizable;
    [self addSubview:button];
    return button;
}

- (NSTextField *)labelAtY:(CGFloat)y {
    NSTextField *label = [NSTextField labelWithString:@""];
    label.frame = NSMakeRect(8, y, 264, 18);
    label.autoresizingMask = NSViewWidthSizable;
    label.font = [NSFont systemFontOfSize:12];
    label.lineBreakMode = NSLineBreakByTruncatingTail;
    [self addSubview:label];
    return label;
}

- (NSSlider *)sliderAtY:(CGFloat)y minimum:(double)minimum maximum:(double)maximum {
    NSSlider *slider = [NSSlider sliderWithValue:minimum minValue:minimum maxValue:maximum
        target:self action:@selector(parametersChanged:)];
    slider.frame = NSMakeRect(8, y, 264, 20);
    slider.autoresizingMask = NSViewWidthSizable;
    slider.continuous = NO;
    [self addSubview:slider];
    return slider;
}

- (instancetype)initWithInfoHandler:(GFTranslationInfoHandler)infoHandler
                      commitHandler:(GFTranslationCommitHandler)commitHandler {
    self = [super initWithFrame:NSMakeRect(0, 0, 280, 220)];
    if (self == nil) { return nil; }
    self.infoHandler = infoHandler;
    self.commitHandler = commitHandler;
    self.enabledButton = [self checkbox:GFLocalized(@"effect.translation.enabled", @"Translation (experimental)") y:190];
    self.enabledButton.toolTip = GFLocalized(@"effect.translation.help", @"Use the saved translation analysis. Disabling keeps the analysis.");
    self.automaticButton = [self checkbox:GFLocalized(@"effect.translation.automatic", @"Automatic parameters") y:164];
    self.referenceLabel = [self labelAtY:140];
    self.referenceSlider = [self sliderAtY:118 minimum:0 maximum:200];
    self.referenceSlider.toolTip = GFLocalized(@"effect.translation.reference_help", @"100% stabilizes the far layer. 0% only compensates retained rotation.");
    self.smoothnessLabel = [self labelAtY:94];
    self.smoothnessSlider = [self sliderAtY:72 minimum:-1 maximum:1];
    self.axisButton = [self checkbox:GFLocalized(@"effect.translation.axis", @"Compensate along the lens axis") y:40];
    self.statusLabel = [self labelAtY:12];
    [self refresh];
    return self;
}

- (void)refresh {
    self.info = self.infoHandler != nil ? self.infoHandler() : (GFTranslationInfo){0};
    GFTranslationParameters p = self.info.parameters;
    self.enabledButton.state = p.enabled ? NSControlStateValueOn : NSControlStateValueOff;
    self.automaticButton.state = p.automatic ? NSControlStateValueOn : NSControlStateValueOff;
    self.axisButton.state = p.along_axis ? NSControlStateValueOn : NSControlStateValueOff;
    self.referenceSlider.doubleValue = p.reference_percent;
    self.smoothnessSlider.doubleValue = log10(MAX(0.1, p.smoothness_seconds));
    self.enabledButton.enabled = self.info.available != 0;
    BOOL enabled = self.info.available && p.enabled;
    self.automaticButton.enabled = enabled;
    self.axisButton.enabled = enabled;
    self.referenceSlider.enabled = enabled && !p.automatic;
    self.smoothnessSlider.enabled = enabled && !p.automatic;
    self.referenceLabel.stringValue = [NSString stringWithFormat:@"%@: %.0f%%",
        GFLocalized(@"effect.translation.reference", @"Reference distance"), p.reference_percent];
    self.smoothnessLabel.stringValue = [NSString stringWithFormat:@"%@: %.2f",
        GFLocalized(@"effect.translation.smoothness", @"Smoothness (s)"), p.smoothness_seconds];
    self.referenceSlider.accessibilityLabel = self.referenceLabel.stringValue;
    self.smoothnessSlider.accessibilityLabel = self.smoothnessLabel.stringValue;
    self.referenceLabel.textColor = self.referenceSlider.enabled ? NSColor.labelColor : NSColor.disabledControlTextColor;
    self.smoothnessLabel.textColor = self.smoothnessSlider.enabled ? NSColor.labelColor : NSColor.disabledControlTextColor;
    self.statusLabel.stringValue = !p.enabled
        ? GFLocalized(@"effect.translation.disabled", @"Translation stabilization disabled")
        : (self.info.stale ? GFLocalized(@"effect.translation.stale", @"Analyze again in Gyroflow")
                          : GFLocalized(@"effect.translation.active", @"Translation stabilization active"));
}

- (void)parametersChanged:(id)sender {
    if (!self.info.available) { return; }
    GFTranslationParameters parameters = self.info.parameters;
    parameters.enabled = self.enabledButton.state == NSControlStateValueOn;
    parameters.automatic = self.automaticButton.state == NSControlStateValueOn;
    parameters.along_axis = self.axisButton.state == NSControlStateValueOn;
    if (sender == self.referenceSlider) { parameters.reference_percent = self.referenceSlider.doubleValue; }
    if (sender == self.smoothnessSlider) { parameters.smoothness_seconds = pow(10, self.smoothnessSlider.doubleValue); }
    parameters.initialized = 1;
    BOOL committed = self.commitHandler != nil && self.commitHandler(parameters, self);
    [self refresh];
    if (!committed) {
        self.statusLabel.stringValue = GFLocalized(@"effect.translation.save_failed", @"Unable to save translation settings");
    }
}
@end
