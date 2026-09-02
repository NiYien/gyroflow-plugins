#import "GFPhase0DropZoneView.h"

#import "GFPhase0Effect.h"
#import "GFPhase0MediaResolver.h"
#import "GFPhase0ProjectStore.h"
#import <os/log.h>
#import <UniformTypeIdentifiers/UniformTypeIdentifiers.h>


static NSArray<NSPasteboardType> *GFPhase0FCPXMLTypes(void) {
    NSMutableArray<NSPasteboardType> *types = [NSMutableArray array];
    for (NSInteger version = 14; version >= 1; version--) {
        [types addObject:[NSString stringWithFormat:@"com.apple.finalcutpro.xml.v1-%ld", (long)version]];
    }
    [types addObject:@"com.apple.finalcutpro.xml"];
    return types;
}


@interface GFPhase0DropZoneView ()
@property(nonatomic, strong) NSTextField *statusLabel;
@property(nonatomic, strong) GFPhase0ProjectStore *projectStore;
@property(nonatomic, copy) GFPhase0ProjectCommitHandler commitHandler;
@property(nonatomic) BOOL dragIsOver;
@end


@implementation GFPhase0DropZoneView

- (instancetype)initWithFrame:(NSRect)frameRect {
    return [self initWithProjectStore:[[GFPhase0ProjectStore alloc] init]
                        commitHandler:^(NSData *projectData, NSView *sender) {
                            (void)projectData;
                            (void)sender;
                        }];
}

- (instancetype)initWithProjectStore:(GFPhase0ProjectStore *)projectStore
                        commitHandler:(GFPhase0ProjectCommitHandler)commitHandler {
    self = [super initWithFrame:NSMakeRect(0, 0, 200, 32)];
    if (self == nil) {
        return nil;
    }
    self.projectStore = projectStore;
    self.commitHandler = commitHandler;

    NSMutableArray<NSPasteboardType> *types = [NSMutableArray arrayWithObject:NSPasteboardTypeFileURL];
    [types addObjectsFromArray:GFPhase0FCPXMLTypes()];
    [types addObject:@"com.apple.flexo.proFFPasteboardUTI"];
    [self registerForDraggedTypes:types];

    self.wantsLayer = YES;
    self.layer.backgroundColor = [[NSColor controlAccentColor] colorWithAlphaComponent:0.12].CGColor;
    self.layer.cornerRadius = 6.0;
    self.layer.borderWidth = 1.0;
    self.layer.borderColor = [NSColor.separatorColor CGColor];

    self.statusLabel = [NSTextField labelWithString:projectStore.status];
    self.statusLabel.frame = NSMakeRect(6, 6, 188, 20);
    self.statusLabel.lineBreakMode = NSLineBreakByTruncatingTail;
    [self addSubview:self.statusLabel];
    return self;
}

- (void)notifyProjectCommitted {
    NSData *data = self.projectStore.currentProjectData;
    if (data != nil && self.commitHandler != nil) {
        self.commitHandler(data, self);
    }
}

- (void)presentProjectPickerForCandidate:(NSURL *)candidate {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    UTType *gyroflowType = [UTType typeWithFilenameExtension:@"gyroflow"];
    panel.allowedContentTypes = @[gyroflowType];
    panel.allowsMultipleSelection = NO;
    panel.canChooseDirectories = NO;
    panel.canChooseFiles = YES;
    panel.canCreateDirectories = NO;
    panel.treatsFilePackagesAsDirectories = YES;
    panel.directoryURL = candidate.URLByDeletingLastPathComponent;
    panel.nameFieldStringValue = candidate.lastPathComponent;
    panel.prompt = @"Grant Access";
    panel.message = @"Select the exact .gyroflow project to grant read access.";

    __weak GFPhase0DropZoneView *weakSelf = self;
    void (^completion)(NSModalResponse) = ^(NSModalResponse response) {
        GFPhase0DropZoneView *strongSelf = weakSelf;
        if (strongSelf == nil) {
            return;
        }
        if (response != NSModalResponseOK || panel.URL == nil) {
            [strongSelf.projectStore recordAuthorizationCancellation];
            strongSelf.statusLabel.stringValue = strongSelf.projectStore.status;
            os_log_info(OS_LOG_DEFAULT,
                        "phase0 project authorization cancelled status=%{public}@",
                        strongSelf.projectStore.status);
            return;
        }

        NSError *importError = nil;
        BOOL loaded = [strongSelf.projectStore importProjectURL:panel.URL error:&importError];
        strongSelf.statusLabel.stringValue = strongSelf.projectStore.status;
        if (loaded) {
            [strongSelf notifyProjectCommitted];
            os_log_info(OS_LOG_DEFAULT,
                        "phase0 project committed selected_url=%{public}@ bytes=%{public}lu version=%{public}@",
                        panel.URL.absoluteString,
                        (unsigned long)strongSelf.projectStore.currentProjectData.length,
                        strongSelf.projectStore.currentProjectVersion);
        } else {
            os_log_error(OS_LOG_DEFAULT,
                         "phase0 project rejected selected_url=%{public}@ reason=%{public}@ status=%{public}@",
                         panel.URL.absoluteString,
                         importError.localizedDescription,
                         strongSelf.projectStore.status);
        }
    };
    [panel beginWithCompletionHandler:completion];
}

- (BOOL)acceptsFirstMouse:(NSEvent *)event {
    return YES;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    NSArray<NSPasteboardType> *availableTypes = sender.draggingPasteboard.types ?: @[];
    BOOL hasFileURL = [availableTypes containsObject:NSPasteboardTypeFileURL];
    BOOL hasFCPXML = NO;
    for (NSPasteboardType type in GFPhase0FCPXMLTypes()) {
        if ([availableTypes containsObject:type]) {
            hasFCPXML = YES;
            break;
        }
    }
    if (!hasFileURL && !hasFCPXML) {
        os_log_info(OS_LOG_DEFAULT,
                    "phase0 drop entered accepted=false pasteboard_types=%{public}@",
                    availableTypes);
        return NSDragOperationNone;
    }
    os_log_info(OS_LOG_DEFAULT,
                "phase0 drop entered accepted=true pasteboard_types=%{public}@",
                availableTypes);
    self.dragIsOver = YES;
    self.layer.borderColor = [NSColor.controlAccentColor CGColor];
    return NSDragOperationCopy;
}

- (void)draggingExited:(id<NSDraggingInfo>)sender {
    os_log_info(OS_LOG_DEFAULT, "phase0 drop exited");
    self.dragIsOver = NO;
    self.layer.borderColor = [NSColor.separatorColor CGColor];
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)sender {
    os_log_info(OS_LOG_DEFAULT, "phase0 drop prepared");
    return YES;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
    NSPasteboard *pasteboard = sender.draggingPasteboard;
    NSArray<NSPasteboardType> *types = pasteboard.types ?: @[];
    NSError *error = nil;
    NSDictionary<NSString *, id> *resolution = nil;
    NSPasteboardType selectedType = nil;
    NSUInteger payloadBytes = 0;

    if ([types containsObject:NSPasteboardTypeFileURL]) {
        selectedType = NSPasteboardTypeFileURL;
        NSArray<NSURL *> *urls = [pasteboard readObjectsForClasses:@[[NSURL class]]
                                                           options:@{NSPasteboardURLReadingFileURLsOnlyKey : @YES}];
        if (urls.count == 1 &&
            [[urls.firstObject.pathExtension lowercaseString] isEqualToString:@"gyroflow"]) {
            BOOL loaded = [self.projectStore importProjectURL:urls.firstObject error:&error];
            self.statusLabel.stringValue = self.projectStore.status;
            if (loaded) {
                [self notifyProjectCommitted];
                os_log_info(OS_LOG_DEFAULT,
                            "phase0 project committed direct_drop_url=%{public}@ bytes=%{public}lu version=%{public}@",
                            urls.firstObject.absoluteString,
                            (unsigned long)self.projectStore.currentProjectData.length,
                            self.projectStore.currentProjectVersion);
            } else {
                os_log_error(OS_LOG_DEFAULT,
                             "phase0 project rejected direct_drop_url=%{public}@ reason=%{public}@ status=%{public}@",
                             urls.firstObject.absoluteString,
                             error.localizedDescription,
                             self.projectStore.status);
            }
            self.layer.borderColor = loaded
                ? [NSColor.separatorColor CGColor]
                : [NSColor.systemRedColor CGColor];
            self.dragIsOver = NO;
            return loaded;
        }
        resolution = GFPhase0ResolveFinderURLs(urls ?: @[], &error);
    } else {
        for (NSPasteboardType type in GFPhase0FCPXMLTypes()) {
            if ([types containsObject:type]) {
                selectedType = type;
                NSData *data = [pasteboard dataForType:type];
                payloadBytes = data.length;
                resolution = GFPhase0ResolveFCPXMLData(data ?: [NSData data], &error);
                NSString *xml = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
                os_log_info(OS_LOG_DEFAULT,
                            "phase0 drop fcpxml_type=%{public}@ bytes=%{public}lu payload=%{public}@",
                            type,
                            (unsigned long)payloadBytes,
                            xml ?: @"<non-UTF-8>");
                break;
            }
        }
    }

    os_log_info(OS_LOG_DEFAULT,
                "phase0 drop pasteboard_types=%{public}@ selected_type=%{public}@ payload_bytes=%{public}lu",
                types,
                selectedType ?: @"none",
                (unsigned long)payloadBytes);
    if (resolution == nil) {
        NSString *message = error.localizedDescription ?: @"unsupported pasteboard payload";
        self.statusLabel.stringValue = [NSString stringWithFormat:@"Rejected: %@", message];
        os_log_error(OS_LOG_DEFAULT, "phase0 drop rejected reason=%{public}@", message);
        self.layer.borderColor = [NSColor.systemRedColor CGColor];
        self.dragIsOver = NO;
        return NO;
    }

    NSURL *projectURL = [NSURL URLWithString:resolution[@"projectURL"]];
    NSString *existence = [resolution[@"projectExists"] boolValue] ? @"exists" : @"missing";
    self.statusLabel.stringValue = [NSString stringWithFormat:@"%@ → %@ (%@)",
                                    resolution[@"source"],
                                    projectURL.lastPathComponent,
                                    existence];
    os_log_info(OS_LOG_DEFAULT,
                "phase0 drop accepted source=%{public}@ media_url=%{public}@ project_url=%{public}@ project_exists=%{public}@",
                resolution[@"source"],
                resolution[@"mediaURL"],
                resolution[@"projectURL"],
                resolution[@"projectExists"]);
    if (![resolution[@"projectExists"] boolValue]) {
        self.statusLabel.stringValue = [NSString stringWithFormat:@"Missing %@; choose .gyroflow",
                                        projectURL.lastPathComponent];
        [self presentProjectPickerForCandidate:projectURL];
    } else {
        NSError *importError = nil;
        BOOL loaded = [self.projectStore importProjectURL:projectURL error:&importError];
        self.statusLabel.stringValue = self.projectStore.status;
        if (loaded) {
            [self notifyProjectCommitted];
            os_log_info(OS_LOG_DEFAULT,
                        "phase0 project committed candidate_url=%{public}@ bytes=%{public}lu version=%{public}@",
                        projectURL.absoluteString,
                        (unsigned long)self.projectStore.currentProjectData.length,
                        self.projectStore.currentProjectVersion);
        } else if ([importError.domain isEqualToString:GFPhase0ProjectStoreErrorDomain] &&
                   importError.code == GFPhase0ProjectStoreErrorReadFailed) {
            os_log_info(OS_LOG_DEFAULT,
                        "phase0 project requires authorization candidate_url=%{public}@ reason=%{public}@",
                        projectURL.absoluteString,
                        importError.localizedDescription);
            [self presentProjectPickerForCandidate:projectURL];
        } else {
            os_log_error(OS_LOG_DEFAULT,
                         "phase0 project rejected candidate_url=%{public}@ reason=%{public}@ status=%{public}@",
                         projectURL.absoluteString,
                         importError.localizedDescription,
                         self.projectStore.status);
        }
    }
    self.layer.borderColor = [NSColor.separatorColor CGColor];
    self.dragIsOver = NO;
    return YES;
}

@end
