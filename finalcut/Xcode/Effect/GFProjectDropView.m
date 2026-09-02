#import "GFProjectDropView.h"

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
@property(nonatomic, strong) NSButton *importButton;
@property(nonatomic, strong) NSTextField *statusLabel;
@property(nonatomic, strong) GFProjectStore *projectStore;
@property(nonatomic, copy) GFProjectPayloadCommitHandler commitHandler;
@end

@implementation GFProjectDropView

- (instancetype)initWithFrame:(NSRect)frameRect {
    return [self initWithProjectStore:[[GFProjectStore alloc] init]
                        commitHandler:^BOOL(NSString *projectPayload, NSView *sender) {
                            (void)projectPayload;
                            (void)sender;
                            return NO;
                        }];
}

- (instancetype)initWithProjectStore:(GFProjectStore *)projectStore
                       commitHandler:(GFProjectPayloadCommitHandler)commitHandler {
    self = [super initWithFrame:NSMakeRect(0, 0, 280, 104)];
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
    self.layer.backgroundColor =
        [[NSColor controlAccentColor] colorWithAlphaComponent:0.10].CGColor;
    self.layer.cornerRadius = 7.0;
    self.layer.borderWidth = 1.0;
    self.layer.borderColor = NSColor.separatorColor.CGColor;

    self.importButton = [[GFImportButton alloc] initWithFrame:NSZeroRect];
    self.importButton.title = @"Import Gyroflow Project";
    self.importButton.target = self;
    self.importButton.action = @selector(importProject:);
    self.importButton.frame = NSMakeRect(8, 68, 264, 28);
    self.importButton.bezelStyle = NSBezelStyleRounded;
    [self addSubview:self.importButton];

    self.statusLabel = [NSTextField wrappingLabelWithString:projectStore.status];
    self.statusLabel.frame = NSMakeRect(8, 7, 264, 56);
    self.statusLabel.maximumNumberOfLines = 3;
    self.statusLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    [self addSubview:self.statusLabel];
    return self;
}

- (void)refreshStatus {
    self.statusLabel.stringValue = self.projectStore.status;
}

- (BOOL)importURL:(NSURL *)url sender:(NSView *)sender allowAuthorizationPanel:(BOOL)allowPanel {
    NSError *error = nil;
    BOOL imported = [self.projectStore
        importProjectURL:url
           commitPayload:^BOOL(NSString *projectPayload) {
               return self.commitHandler != nil &&
                   self.commitHandler(projectPayload, sender);
           }
                   error:&error];
    if (imported) {
        [self refreshStatus];
        self.layer.borderColor = NSColor.separatorColor.CGColor;
        return YES;
    }
    [self refreshStatus];
    self.layer.borderColor = NSColor.systemRedColor.CGColor;
    if (allowPanel &&
        [error.domain isEqualToString:GFProjectStoreErrorDomain] &&
        error.code == GFProjectStoreErrorReadFailed) {
        [self presentProjectPickerForCandidate:url authorizationOnly:YES];
    }
    return NO;
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
    panel.prompt = authorizationOnly ? @"Grant Access" : @"Import";
    panel.message = authorizationOnly
        ? @"Select the suggested .gyroflow project to grant read access."
        : @"Choose a Gyroflow project. Video files are not accepted.";
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
    [self presentProjectPickerForCandidate:nil authorizationOnly:NO];
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    return YES;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    NSArray<NSPasteboardType> *types = sender.draggingPasteboard.types ?: @[];
    BOOL accepted = [types containsObject:NSPasteboardTypeFileURL];
    for (NSPasteboardType type in GFFCPXMLPasteboardTypes()) {
        accepted = accepted || [types containsObject:type];
    }
    if (!accepted) {
        return NSDragOperationNone;
    }
    self.layer.borderColor = NSColor.controlAccentColor.CGColor;
    return NSDragOperationCopy;
}

- (void)draggingExited:(id<NSDraggingInfo>)sender {
    self.layer.borderColor = NSColor.separatorColor.CGColor;
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)sender {
    return YES;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
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
        self.statusLabel.stringValue = [NSString stringWithFormat:
            @"Could not locate a project: %@; use Import Gyroflow Project",
            error.localizedDescription ?: @"unsupported drop"];
        self.layer.borderColor = NSColor.systemRedColor.CGColor;
        return NO;
    }
    NSURL *projectURL = [NSURL URLWithString:resolution[@"projectURL"]];
    if (![resolution[@"projectExists"] boolValue]) {
        self.statusLabel.stringValue = [NSString stringWithFormat:
            @"No sibling %@ found; use Import Gyroflow Project",
            projectURL.lastPathComponent];
        self.layer.borderColor = NSColor.systemRedColor.CGColor;
        return NO;
    }
    return [self importURL:projectURL
                    sender:self
   allowAuthorizationPanel:YES];
}

@end
