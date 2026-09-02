#import "GFPhase0WorkflowViewController.h"

#import <CoreMedia/CoreMedia.h>
#import <ProExtension/ProExtension.h>
#import <ProExtensionHost/ProExtensionHost.h>
#import <os/log.h>


static NSArray<NSPasteboardType> *GFPhase0WorkflowFCPXMLTypes(void) {
    NSMutableArray<NSPasteboardType> *types = [NSMutableArray array];
    for (NSInteger version = 14; version >= 1; version--) {
        [types addObject:[NSString stringWithFormat:@"com.apple.finalcutpro.xml.v1-%ld",
                                                    (long)version]];
    }
    [types addObject:@"com.apple.finalcutpro.xml"];
    return types;
}


static NSString *GFPhase0WorkflowTimeString(CMTime time) {
    if (!CMTIME_IS_VALID(time)) {
        return @"invalid";
    }
    return [NSString stringWithFormat:@"%lld/%d (%.6fs)",
                                      time.value,
                                      time.timescale,
                                      CMTimeGetSeconds(time)];
}


@interface GFPhase0WorkflowDropView : NSView
@property(nonatomic, strong) NSTextField *statusLabel;
@end


@implementation GFPhase0WorkflowDropView

- (instancetype)initWithFrame:(NSRect)frameRect {
    self = [super initWithFrame:frameRect];
    if (self == nil) {
        return nil;
    }
    [self registerForDraggedTypes:GFPhase0WorkflowFCPXMLTypes()];
    self.wantsLayer = YES;
    self.layer.backgroundColor = [[NSColor controlAccentColor] colorWithAlphaComponent:0.14].CGColor;
    self.layer.cornerRadius = 8.0;
    self.layer.borderWidth = 1.0;
    self.layer.borderColor = NSColor.separatorColor.CGColor;

    self.statusLabel = [NSTextField labelWithString:@"Drop one Final Cut project or clip FCPXML here"];
    self.statusLabel.frame = NSMakeRect(12.0, 22.0, frameRect.size.width - 24.0, 22.0);
    self.statusLabel.alignment = NSTextAlignmentCenter;
    self.statusLabel.lineBreakMode = NSLineBreakByTruncatingMiddle;
    self.statusLabel.autoresizingMask = NSViewWidthSizable;
    [self addSubview:self.statusLabel];
    return self;
}

- (NSDragOperation)draggingEntered:(id<NSDraggingInfo>)sender {
    NSArray<NSPasteboardType> *types = sender.draggingPasteboard.types ?: @[];
    for (NSPasteboardType type in GFPhase0WorkflowFCPXMLTypes()) {
        if ([types containsObject:type]) {
            self.layer.borderColor = NSColor.controlAccentColor.CGColor;
            os_log_info(OS_LOG_DEFAULT,
                        "phase0 workflow drop entered accepted=true pasteboard_types=%{public}@",
                        types);
            return NSDragOperationCopy;
        }
    }
    os_log_info(OS_LOG_DEFAULT,
                "phase0 workflow drop entered accepted=false pasteboard_types=%{public}@",
                types);
    return NSDragOperationNone;
}

- (void)draggingExited:(id<NSDraggingInfo>)sender {
    (void)sender;
    self.layer.borderColor = NSColor.separatorColor.CGColor;
}

- (BOOL)prepareForDragOperation:(id<NSDraggingInfo>)sender {
    (void)sender;
    return YES;
}

- (BOOL)performDragOperation:(id<NSDraggingInfo>)sender {
    NSPasteboard *pasteboard = sender.draggingPasteboard;
    NSPasteboardType selectedType = nil;
    NSData *payload = nil;
    for (NSPasteboardType type in GFPhase0WorkflowFCPXMLTypes()) {
        if ([pasteboard.types containsObject:type]) {
            selectedType = type;
            payload = [pasteboard dataForType:type];
            break;
        }
    }
    if (payload.length == 0) {
        self.statusLabel.stringValue = @"Rejected: no FCPXML payload";
        self.layer.borderColor = NSColor.systemRedColor.CGColor;
        return NO;
    }

    NSError *parseError = nil;
    NSXMLDocument *document = [[NSXMLDocument alloc] initWithData:payload
                                                          options:0
                                                            error:&parseError];
    if (document == nil) {
        self.statusLabel.stringValue = [NSString stringWithFormat:@"Rejected XML: %@",
                                                                  parseError.localizedDescription];
        self.layer.borderColor = NSColor.systemRedColor.CGColor;
        return NO;
    }

    NSError *queryError = nil;
    NSArray<NSXMLNode *> *projects = [document nodesForXPath:@"//project" error:&queryError];
    NSArray<NSXMLNode *> *assetClips = [document nodesForXPath:@"//asset-clip" error:&queryError];
    NSArray<NSXMLNode *> *refClips = [document nodesForXPath:@"//ref-clip" error:&queryError];
    NSArray<NSXMLNode *> *assets = [document nodesForXPath:@"//asset" error:&queryError];
    NSArray<NSXMLNode *> *filters = [document nodesForXPath:@"//filter-video" error:&queryError];
    NSArray<NSXMLNode *> *parameters = [document nodesForXPath:@"//filter-video/param" error:&queryError];
    if (queryError != nil) {
        self.statusLabel.stringValue = [NSString stringWithFormat:@"Rejected XPath: %@",
                                                                  queryError.localizedDescription];
        self.layer.borderColor = NSColor.systemRedColor.CGColor;
        return NO;
    }

    NSURL *applicationSupport = [[NSFileManager defaultManager]
        URLsForDirectory:NSApplicationSupportDirectory
               inDomains:NSUserDomainMask].firstObject;
    NSURL *captureDirectory = [applicationSupport URLByAppendingPathComponent:@"GyroflowNiYienPhase0Workflow"
                                                                  isDirectory:YES];
    NSError *captureError = nil;
    [[NSFileManager defaultManager] createDirectoryAtURL:captureDirectory
                             withIntermediateDirectories:YES
                                              attributes:nil
                                                   error:&captureError];
    NSURL *captureURL = [captureDirectory URLByAppendingPathComponent:@"last-drop.fcpxml"];
    BOOL captured = captureError == nil &&
        [payload writeToURL:captureURL options:NSDataWritingAtomic error:&captureError];

    NSMutableArray<NSString *> *parameterSummaries = [NSMutableArray array];
    for (NSXMLNode *node in parameters) {
        NSXMLElement *element = (NSXMLElement *)node;
        NSString *name = [[element attributeForName:@"name"] stringValue] ?: @"";
        NSString *key = [[element attributeForName:@"key"] stringValue] ?: @"";
        NSString *value = [[element attributeForName:@"value"] stringValue] ?: @"";
        [parameterSummaries addObject:[NSString stringWithFormat:@"name=%@ key=%@ value_bytes=%lu",
                                                                 name,
                                                                 key,
                                                                 (unsigned long)value.length]];
    }

    NSString *version = [[document.rootElement attributeForName:@"version"] stringValue] ?: @"unknown";
    NSString *summary = [NSString stringWithFormat:@"FCPXML %@: projects=%lu asset-clips=%lu ref-clips=%lu assets=%lu filters=%lu params=%lu bytes=%lu capture=%@",
                                                   version,
                                                   (unsigned long)projects.count,
                                                   (unsigned long)assetClips.count,
                                                   (unsigned long)refClips.count,
                                                   (unsigned long)assets.count,
                                                   (unsigned long)filters.count,
                                                   (unsigned long)parameters.count,
                                                   (unsigned long)payload.length,
                                                   captured ? @"ok" : @"failed"];
    self.statusLabel.stringValue = summary;
    self.layer.borderColor = NSColor.systemGreenColor.CGColor;
    os_log_info(OS_LOG_DEFAULT,
                "phase0 workflow fcpxml accepted type=%{public}@ summary=%{public}@",
                selectedType,
                summary);
    os_log_info(OS_LOG_DEFAULT,
                "phase0 workflow fcpxml capture_url=%{public}@ capture_error=%{public}@ filter_params=%{public}@",
                captureURL.absoluteString,
                captureError.localizedDescription ?: @"none",
                parameterSummaries);
    return YES;
}

@end


@interface GFPhase0WorkflowViewController () <FCPXTimelineObserver>
@property(nonatomic, strong) id<FCPXHost> host;
@property(nonatomic, strong) FCPXTimeline *timeline;
@property(nonatomic, strong) NSTextField *hostLabel;
@property(nonatomic, strong) NSTextField *sequenceLabel;
@property(nonatomic, strong) NSTextField *playheadLabel;
@property(nonatomic, strong) NSTextField *rangeLabel;
@end


@implementation GFPhase0WorkflowViewController

- (NSTextField *)labelWithFrame:(NSRect)frame text:(NSString *)text {
    NSTextField *label = [NSTextField wrappingLabelWithString:text];
    label.frame = frame;
    label.autoresizingMask = NSViewWidthSizable;
    return label;
}

- (void)loadView {
    NSView *root = [[NSView alloc] initWithFrame:NSMakeRect(0.0, 0.0, 520.0, 340.0)];
    self.view = root;

    NSTextField *title = [NSTextField labelWithString:@"Gyroflow NiYien Phase 0 Workflow Probe"];
    title.font = [NSFont boldSystemFontOfSize:16.0];
    title.frame = NSMakeRect(20.0, 302.0, 480.0, 24.0);
    title.autoresizingMask = NSViewWidthSizable;
    [root addSubview:title];

    self.hostLabel = [self labelWithFrame:NSMakeRect(20.0, 270.0, 480.0, 24.0)
                                     text:@"Host: waiting for proxy"];
    self.sequenceLabel = [self labelWithFrame:NSMakeRect(20.0, 226.0, 480.0, 40.0)
                                         text:@"Active sequence: waiting for observer callback"];
    self.playheadLabel = [self labelWithFrame:NSMakeRect(20.0, 198.0, 480.0, 24.0)
                                         text:@"Playhead: waiting for observer callback"];
    self.rangeLabel = [self labelWithFrame:NSMakeRect(20.0, 170.0, 480.0, 24.0)
                                      text:@"Sequence range: waiting for observer callback"];
    [root addSubview:self.hostLabel];
    [root addSubview:self.sequenceLabel];
    [root addSubview:self.playheadLabel];
    [root addSubview:self.rangeLabel];

    NSTextField *boundary = [self labelWithFrame:NSMakeRect(20.0, 132.0, 480.0, 34.0)
                                            text:@"SDK proxy boundary: active sequence, sequence range, and playhead only; no clip enumeration API."];
    boundary.textColor = NSColor.secondaryLabelColor;
    [root addSubview:boundary];

    GFPhase0WorkflowDropView *dropView = [[GFPhase0WorkflowDropView alloc]
        initWithFrame:NSMakeRect(20.0, 42.0, 480.0, 74.0)];
    dropView.autoresizingMask = NSViewWidthSizable;
    [root addSubview:dropView];
}

- (void)viewDidAppear {
    [super viewDidAppear];
    id<NSObject> hostObject = ProExtensionHostSingleton();
    if (hostObject == nil || ![hostObject conformsToProtocol:@protocol(FCPXHost)]) {
        self.hostLabel.stringValue = @"Host: unavailable or incompatible";
        os_log_error(OS_LOG_DEFAULT, "phase0 workflow host proxy unavailable");
        return;
    }
    self.host = (id<FCPXHost>)hostObject;
    self.timeline = self.host.timeline;
    self.hostLabel.stringValue = [NSString stringWithFormat:@"Host: %@ %@ (%@)",
                                                            self.host.name,
                                                            self.host.versionString,
                                                            self.host.bundleIdentifier];
    [self.timeline addTimelineObserver:self];
    os_log_info(OS_LOG_DEFAULT,
                "phase0 workflow host connected name=%{public}@ version=%{public}@ bundle_id=%{public}@ timeline=%{public}@",
                self.host.name,
                self.host.versionString,
                self.host.bundleIdentifier,
                self.timeline);
    [self refreshLiveProxy];
}

- (void)viewWillDisappear {
    [self.timeline removeTimelineObserver:self];
    self.timeline = nil;
    self.host = nil;
    [super viewWillDisappear];
}

- (void)refreshLiveProxy {
    FCPXSequence *sequence = self.timeline.activeSequence;
    if (sequence == nil) {
        self.sequenceLabel.stringValue = @"Active sequence: nil";
    } else {
        self.sequenceLabel.stringValue = [NSString stringWithFormat:@"Active sequence: %@ start=%@ duration=%@ frame=%@ format=%ld",
                                                                    sequence.name,
                                                                    GFPhase0WorkflowTimeString(sequence.startTime),
                                                                    GFPhase0WorkflowTimeString(sequence.duration),
                                                                    GFPhase0WorkflowTimeString(sequence.frameDuration),
                                                                    (long)sequence.timecodeFormat];
    }
    self.playheadLabel.stringValue = [NSString stringWithFormat:@"Playhead: %@",
                                                               GFPhase0WorkflowTimeString(self.timeline.playheadTime)];
    CMTimeRange range = self.timeline.sequenceTimeRange;
    self.rangeLabel.stringValue = [NSString stringWithFormat:@"Sequence range: start=%@ duration=%@",
                                                            GFPhase0WorkflowTimeString(range.start),
                                                            GFPhase0WorkflowTimeString(range.duration)];
    os_log_info(OS_LOG_DEFAULT,
                "phase0 workflow live proxy sequence=%{public}@ playhead=%{public}@ range_start=%{public}@ range_duration=%{public}@",
                sequence.name ?: @"nil",
                GFPhase0WorkflowTimeString(self.timeline.playheadTime),
                GFPhase0WorkflowTimeString(range.start),
                GFPhase0WorkflowTimeString(range.duration));
}

- (void)refreshLiveProxyOnMainQueue {
    dispatch_async(dispatch_get_main_queue(), ^{
        [self refreshLiveProxy];
    });
}

- (void)activeSequenceChanged {
    [self refreshLiveProxyOnMainQueue];
}

- (void)playheadTimeChanged {
    [self refreshLiveProxyOnMainQueue];
}

- (void)sequenceTimeRangeChanged {
    [self refreshLiveProxyOnMainQueue];
}

@end
