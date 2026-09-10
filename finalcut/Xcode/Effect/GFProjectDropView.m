#import "GFProjectDropView.h"

#import "GFLocalization.h"
#import "GFProjectStore.h"
#import "GFSourceResolver.h"
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>

static NSArray<NSPasteboardType> *GFFCPXMLPasteboardTypes(void) {
    NSMutableArray<NSPasteboardType> *types = [NSMutableArray array];
    for (NSInteger version = 14; version >= 1; version--) {
        [types addObject:
            [NSString stringWithFormat:@"com.apple.finalcutpro.xml.v1-%ld", (long)version]];
    }
    [types addObject:@"com.apple.finalcutpro.xml"];
    return types;
}

@interface GFImportButton : NSButton
@end

@implementation GFImportButton

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return YES;
}

- (BOOL)accessibilityPerformPress {
    if (self.action != NULL && self.target != nil) {
        return [NSApp sendAction:self.action to:self.target from:self];
    }
    return [super accessibilityPerformPress];
}

@end

@interface GFProjectDropView ()
@property(nonatomic, strong) NSButton *loadButton;
@property(nonatomic, strong) NSTextField *statusLabel;
@property(nonatomic, strong) GFProjectStore *projectStore;
@property(nonatomic, copy) GFProjectImportCommitHandler commitHandler;
@property(nonatomic) NSUInteger importGeneration;
@property(nonatomic) BOOL importInFlight;
@end

@implementation GFProjectDropView

- (instancetype)initWithFrame:(NSRect)frameRect {
    return [self initWithProjectStore:[[GFProjectStore alloc] init]
                        commitHandler:^BOOL(GFProjectImportCandidate *candidate, NSView *sender) {
                            (void)candidate;
                            (void)sender;
                            return NO;
                        }];
}

- (instancetype)initWithProjectStore:(GFProjectStore *)projectStore
                       commitHandler:(GFProjectImportCommitHandler)commitHandler {
    self = [super initWithFrame:NSMakeRect(0, 0, 280, 56)];
    if (self == nil) {
        return nil;
    }
    self.projectStore = projectStore;
    self.commitHandler = commitHandler;
    NSMutableArray<NSPasteboardType> *types =
        [NSMutableArray arrayWithObject:NSPasteboardTypeFileURL];
    [types addObjectsFromArray:GFFCPXMLPasteboardTypes()];
    [self registerForDraggedTypes:types];
    self.wantsLayer = YES;
    self.layer.cornerRadius = 4.0;

    self.statusLabel = [NSTextField labelWithString:@""];
    self.statusLabel.frame = NSMakeRect(8, 34, 264, 18);
    self.statusLabel.autoresizingMask = NSViewWidthSizable;
    self.statusLabel.font = [NSFont systemFontOfSize:12.0];
    self.statusLabel.maximumNumberOfLines = 1;
    self.statusLabel.lineBreakMode = NSLineBreakByTruncatingMiddle;
    self.statusLabel.selectable = YES;
    [self addSubview:self.statusLabel];

    self.loadButton = [[GFImportButton alloc] initWithFrame:NSMakeRect(8, 2, 264, 26)];
    self.loadButton.autoresizingMask = NSViewWidthSizable;
    self.loadButton.title = GFLocalized(@"effect.action.load_project", @"Load .gyroflow…");
    self.loadButton.bezelStyle = NSBezelStyleRounded;
    self.loadButton.target = self;
    self.loadButton.action = @selector(importProject:);
    self.loadButton.toolTip = GFLocalized(@"effect.action.load_project_help",
        @"Load a .gyroflow project. Complex edits can be prepared in Gyroflow and imported here.");
    self.loadButton.accessibilityHelp = self.loadButton.toolTip;
    [self addSubview:self.loadButton];

    self.accessibilityHelp = GFLocalized(@"effect.inspector.help",
        @"Load a .gyroflow project. Complex edits should be made in Gyroflow, then loaded here.");
    [self refreshStatus];
    return self;
}

- (void)refreshStatus {
    NSString *name = self.projectStore.currentProjectName;
    if (name.length == 0) {
        name = @"Gyroflow";
    }
    BOOL loaded = self.projectStore.currentProjectPayload.length > 0;
    self.statusLabel.stringValue = [NSString stringWithFormat:loaded
        ? GFLocalized(@"effect.status.project_loaded", @"%@ loaded")
        : GFLocalized(@"effect.status.project_unloaded", @"%@ not loaded"), name];
    self.loadButton.enabled = !self.importInFlight;
    self.statusLabel.accessibilityLabel = self.statusLabel.stringValue;
    self.accessibilityLabel = self.statusLabel.stringValue;
    [self showStatusDetail:self.projectStore.status];
}

- (void)showStatusDetail:(NSString *)detail {
    self.statusLabel.toolTip = detail;
    self.statusLabel.accessibilityValue = detail;
}

- (BOOL)importURL:(NSURL *)url sender:(NSView *)sender allowAuthorizationPanel:(BOOL)allowPanel {
    NSAssert(NSThread.isMainThread, @"Project imports must be scheduled on the main thread");
    self.importGeneration += 1;
    NSUInteger generation = self.importGeneration;
    self.importInFlight = YES;
    self.loadButton.enabled = NO;
    [self showStatusDetail:[NSString stringWithFormat:GFLocalized(
        @"effect.status.loading_project", @"Loading %@…"), url.lastPathComponent]];

    GFProjectStore *store = self.projectStore;
    NSURL *preparedURL = [url copy];
    __weak GFProjectDropView *weakSelf = self;
    dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
        NSError *preparationError = nil;
        GFProjectImportCandidate *candidate =
            [store prepareImportProjectURL:preparedURL error:&preparationError];
        dispatch_async(dispatch_get_main_queue(), ^{
            GFProjectDropView *strongSelf = weakSelf;
            if (strongSelf == nil || strongSelf.importGeneration != generation) {
                return;
            }
            strongSelf.importInFlight = NO;
            if (candidate == nil) {
                NSError *resolvedError = preparationError
                    ?: [NSError errorWithDomain:GFProjectStoreErrorDomain
                                            code:GFProjectStoreErrorInvalidProject
                                        userInfo:@{
                                            NSLocalizedDescriptionKey : GFLocalized(
                                                @"effect.error.validation_failed",
                                                @"Project validation failed")
                                        }];
                [store recordImportFailureForURL:preparedURL error:resolvedError];
                [strongSelf refreshStatus];
                if (allowPanel &&
                    [resolvedError.domain isEqualToString:GFProjectStoreErrorDomain] &&
                    resolvedError.code == GFProjectStoreErrorReadFailed) {
                    [strongSelf presentProjectPickerForCandidate:preparedURL
                                               authorizationOnly:YES];
                }
                return;
            }

            NSError *commitError = nil;
            [store commitPreparedImportCandidate:candidate
                                       sourceURL:preparedURL
                                  commitCandidate:^BOOL(GFProjectImportCandidate *prepared) {
                                      return strongSelf.commitHandler != nil &&
                                          strongSelf.commitHandler(prepared, sender);
                                  }
                                            error:&commitError];
            [strongSelf refreshStatus];
        });
    });
    return YES;
}

- (void)presentProjectPickerForCandidate:(nullable NSURL *)candidate
                       authorizationOnly:(BOOL)authorizationOnly {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    UTType *gyroflowType = [UTType typeWithFilenameExtension:@"gyroflow"];
    panel.allowedContentTypes = @[gyroflowType];
    panel.allowsMultipleSelection = NO;
    panel.canChooseDirectories = NO;
    panel.canChooseFiles = YES;
    panel.canCreateDirectories = NO;
    panel.treatsFilePackagesAsDirectories = YES;
    panel.prompt = authorizationOnly
        ? GFLocalized(@"effect.panel.grant_title", @"Grant Access")
        : GFLocalized(@"effect.panel.load_title", @"Load");
    panel.message = authorizationOnly
        ? GFLocalized(@"effect.panel.grant_message",
            @"Select the suggested .gyroflow project to grant read access.")
        : GFLocalized(@"effect.panel.load_message",
            @"Choose a Gyroflow project. Video files are not accepted.");
    if (candidate != nil) {
        panel.directoryURL = candidate.URLByDeletingLastPathComponent;
        panel.nameFieldStringValue = candidate.lastPathComponent;
    }

    __weak GFProjectDropView *weakSelf = self;
    // FxPlug view services are not the active application, so bring the
    // user-requested picker forward instead of leaving it behind Final Cut.
    NSApplicationActivationPolicy previousActivationPolicy = NSApp.activationPolicy;
    [NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [NSApp activateIgnoringOtherApps:YES];
    [panel beginWithCompletionHandler:^(NSModalResponse response) {
        [NSApp setActivationPolicy:previousActivationPolicy];
        GFProjectDropView *strongSelf = weakSelf;
        if (strongSelf == nil) {
            return;
        }
        if (response != NSModalResponseOK || panel.URL == nil) {
            [strongSelf.projectStore recordAuthorizationCancellation];
            [strongSelf refreshStatus];
            return;
        }
        [strongSelf importURL:panel.URL
                       sender:strongSelf
      allowAuthorizationPanel:NO];
    }];
}

- (void)importProject:(id)sender {
    (void)sender;
    if (self.importInFlight) {
        return;
    }
    [self presentProjectPickerForCandidate:nil authorizationOnly:NO];
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    (void)event;
    return YES;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    if (self.importInFlight) {
        return NSDragOperationNone;
    }
    NSArray<NSPasteboardType> *types = sender.draggingPasteboard.types ?: @[];
    BOOL accepted = [types containsObject:NSPasteboardTypeFileURL];
    for (NSPasteboardType type in GFFCPXMLPasteboardTypes()) {
        accepted = accepted || [types containsObject:type];
    }
    if (!accepted) {
        return NSDragOperationNone;
    }
    self.layer.borderWidth = 1.5;
    self.layer.borderColor = NSColor.controlAccentColor.CGColor;
    return NSDragOperationCopy;
}

- (void)draggingExited:(id<NSDraggingInfo>)sender {
    (void)sender;
    self.layer.borderWidth = 0.0;
    self.layer.borderColor = NSColor.separatorColor.CGColor;
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)sender {
    return YES;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
    self.layer.borderWidth = 0.0;
    self.layer.borderColor = NSColor.separatorColor.CGColor;
    NSPasteboard *pasteboard = sender.draggingPasteboard;
    NSArray<NSPasteboardType> *types = pasteboard.types ?: @[];
    NSError *error = nil;
    NSDictionary<NSString *, id> *resolution = nil;
    if ([types containsObject:NSPasteboardTypeFileURL]) {
        NSArray<NSURL *> *urls = [pasteboard readObjectsForClasses:@[[NSURL class]]
                                                           options:@{
                                                               NSPasteboardURLReadingFileURLsOnlyKey :
                                                                   @YES
                                                           }];
        if (urls.count == 1 &&
            [[urls.firstObject.pathExtension lowercaseString] isEqualToString:@"gyroflow"]) {
            return [self importURL:urls.firstObject
                            sender:self
           allowAuthorizationPanel:NO];
        }
        resolution = GFResolveFinderURLs(urls ?: @[], &error);
    } else {
        for (NSPasteboardType type in GFFCPXMLPasteboardTypes()) {
            if ([types containsObject:type]) {
                NSData *data = [pasteboard dataForType:type];
                resolution = GFResolveFCPXMLData(data ?: [NSData data], &error);
                break;
            }
        }
    }
    if (resolution == nil) {
        [self showStatusDetail:[NSString stringWithFormat:GFLocalized(
            @"effect.drop.locate_failed",
            @"Could not locate a project: %@; use Browse…"),
            error.localizedDescription
                ?: GFLocalized(@"effect.error.unknown", @"unknown error")]];
        return NO;
    }
    NSURL *projectURL = [NSURL URLWithString:resolution[@"projectURL"]];
    if (![resolution[@"projectExists"] boolValue]) {
        [self showStatusDetail:[NSString stringWithFormat:GFLocalized(
            @"effect.drop.sibling_missing",
            @"No sibling %@ found; use Browse…"),
            projectURL.lastPathComponent]];
        return NO;
    }
    return [self importURL:projectURL
                    sender:self
   allowAuthorizationPanel:YES];
}

@end
