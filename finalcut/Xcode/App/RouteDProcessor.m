#import "RouteDProcessor.h"

#import "GFLocalization.h"
#import "GyroflowFinalCut.h"

static NSString *const GFRouteDProcessorErrorDomain =
    @"com.niyien.gyroflow.finalcut.route-d";
static const unsigned long long GFMaximumProjectBytes = 256ULL * 1024ULL * 1024ULL;

@interface GFRouteDProjectSnapshot : NSObject
@property(nonatomic, readonly) NSData *pathData;
@property(nonatomic, readonly, nullable) NSData *projectData;
@property(nonatomic, readonly) GFRouteDProjectInputStatus status;
- (instancetype)initWithPath:(NSString *)path
                  projectData:(nullable NSData *)projectData
                       status:(GFRouteDProjectInputStatus)status;
@end

@implementation GFRouteDProjectSnapshot

- (instancetype)initWithPath:(NSString *)path
                  projectData:(nullable NSData *)projectData
                       status:(GFRouteDProjectInputStatus)status {
    self = [super init];
    if (self != nil) {
        _pathData = [path dataUsingEncoding:NSUTF8StringEncoding];
        _projectData = [projectData copy];
        _status = status;
    }
    return self;
}

@end

static NSInteger GFRouteDInputPriority(GFRouteDProjectInputStatus status) {
    switch (status) {
        case GF_ROUTE_D_PROJECT_INPUT_AVAILABLE:
            return 4;
        case GF_ROUTE_D_PROJECT_INPUT_TOO_LARGE:
            return 3;
        case GF_ROUTE_D_PROJECT_INPUT_MISSING:
            return 2;
        default:
            return 1;
    }
}

static BOOL GFRouteDErrorIsMissing(NSError *error) {
    return [error.domain isEqualToString:NSCocoaErrorDomain] &&
        (error.code == NSFileNoSuchFileError ||
         error.code == NSFileReadNoSuchFileError);
}

static GFRouteDProjectSnapshot *GFReadProject(
    NSURL *projectURL,
    NSString *expectedPath
) {
    NSError *resourceError = nil;
    NSDictionary<NSURLResourceKey, id> *values = [projectURL resourceValuesForKeys:@[
        NSURLIsRegularFileKey,
        NSURLIsSymbolicLinkKey,
        NSURLFileSizeKey,
    ] error:&resourceError];
    if (resourceError != nil) {
        GFRouteDProjectInputStatus status = GFRouteDErrorIsMissing(resourceError)
            ? GF_ROUTE_D_PROJECT_INPUT_MISSING
            : GF_ROUTE_D_PROJECT_INPUT_PERMISSION_DENIED;
        return [[GFRouteDProjectSnapshot alloc] initWithPath:expectedPath
                                                 projectData:nil
                                                      status:status];
    }
    if ([values[NSURLIsSymbolicLinkKey] boolValue] ||
        ![values[NSURLIsRegularFileKey] boolValue]) {
        return [[GFRouteDProjectSnapshot alloc]
            initWithPath:expectedPath
             projectData:nil
                  status:GF_ROUTE_D_PROJECT_INPUT_PERMISSION_DENIED];
    }
    if ([values[NSURLFileSizeKey] unsignedLongLongValue] > GFMaximumProjectBytes) {
        return [[GFRouteDProjectSnapshot alloc]
            initWithPath:expectedPath
             projectData:nil
                  status:GF_ROUTE_D_PROJECT_INPUT_TOO_LARGE];
    }
    NSError *readError = nil;
    NSData *projectData = [NSData dataWithContentsOfURL:projectURL
                                                options:NSDataReadingMappedIfSafe
                                                  error:&readError];
    GFRouteDProjectInputStatus status = projectData != nil
        ? GF_ROUTE_D_PROJECT_INPUT_AVAILABLE
        : (GFRouteDErrorIsMissing(readError)
            ? GF_ROUTE_D_PROJECT_INPUT_MISSING
            : GF_ROUTE_D_PROJECT_INPUT_PERMISSION_DENIED);
    return [[GFRouteDProjectSnapshot alloc] initWithPath:expectedPath
                                             projectData:projectData
                                                  status:status];
}

static NSArray<GFRouteDProjectSnapshot *> *GFProjectSnapshotsFromFCPXML(
    NSData *input,
    NSURL *documentURL
) {
    (void)documentURL;
    NSError *xmlError = nil;
    NSXMLDocument *document = [[NSXMLDocument alloc]
        initWithData:input
             options:NSXMLNodeLoadExternalEntitiesNever
               error:&xmlError];
    if (document == nil) {
        return @[];
    }
    NSArray<NSXMLNode *> *nodes = [document nodesForXPath:
        @"//media-rep[@kind='original-media']" error:&xmlError];
    NSMutableDictionary<NSString *, GFRouteDProjectSnapshot *> *snapshots =
        [NSMutableDictionary dictionary];
    for (NSXMLNode *node in nodes) {
        if (![node isKindOfClass:[NSXMLElement class]]) {
            continue;
        }
        NSXMLElement *mediaRepresentation = (NSXMLElement *)node;
        NSString *source = [mediaRepresentation attributeForName:@"src"].stringValue;
        NSURL *sourceURL = source.length > 0 ? [NSURL URLWithString:source] : nil;
        if (!sourceURL.isFileURL || sourceURL.path.length == 0) {
            continue;
        }
        NSURL *projectURL = [[[sourceURL URLByDeletingPathExtension]
            URLByAppendingPathExtension:@"gyroflow"] URLByStandardizingPath];
        NSString *expectedPath = projectURL.path;
        GFRouteDProjectSnapshot *existing = snapshots[expectedPath];
        if (existing != nil &&
            existing.status != GF_ROUTE_D_PROJECT_INPUT_PERMISSION_DENIED) {
            continue;
        }
        GFRouteDProjectSnapshot *snapshot = GFReadProject(projectURL, expectedPath);
        existing = snapshots[expectedPath];
        if (existing == nil ||
            GFRouteDInputPriority(snapshot.status) > GFRouteDInputPriority(existing.status)) {
            snapshots[expectedPath] = snapshot;
        }
    }
    return snapshots.allValues;
}

static NSString *GFLocalizedRouteDError(GFStatus status) {
    switch (status) {
        case GF_STATUS_ROUTE_D_INVALID_INPUT:
            return GFLocalized(@"app.error.route.invalid_input",
                               @"The selected Final Cut project XML is invalid.");
        case GF_STATUS_ROUTE_D_UNSAFE_STRUCTURE:
            return GFLocalized(@"app.error.route.unsafe_structure",
                               @"The Final Cut project has an ambiguous or unsafe structure.");
        case GF_STATUS_ROUTE_D_NO_UPDATEABLE_TARGETS:
            return GFLocalized(@"app.error.route.no_targets",
                               @"The batch did not update any replacement targets.");
        default:
            return GFLocalized(@"app.error.route.process",
                               @"Unable to process Final Cut project XML");
    }
}

@implementation RouteDProcessor

+ (nullable NSDictionary<NSString *, NSData *> *)resultForStatus:(GFStatus)status
                                                           result:(GFRouteDPatchResult *)result
                                                      bridgeError:(GFError *)bridgeError
                                                            error:(NSError **)error {
    if (status != GF_STATUS_OK) {
        NSString *diagnostic = bridgeError != NULL && bridgeError->message != NULL
            ? [NSString stringWithUTF8String:bridgeError->message]
            : nil;
        NSString *message = GFLocalizedRouteDError(status);
        if (error != NULL) {
            NSMutableDictionary *userInfo = [@{
                NSLocalizedDescriptionKey : message
                    ?: GFLocalized(@"app.error.route.unknown",
                                   @"Unknown batch-processing error")
            } mutableCopy];
            if (diagnostic.length > 0) {
                userInfo[NSDebugDescriptionErrorKey] = diagnostic;
            }
            *error = [NSError errorWithDomain:GFRouteDProcessorErrorDomain
                                         code:status
                                     userInfo:userInfo];
        }
        gf_finalcut_error_free(bridgeError);
        gf_finalcut_route_d_patch_result_free(result);
        return nil;
    }
    NSData *fcpxml = [NSData dataWithBytes:result->fcpxml.data
                                    length:result->fcpxml.len];
    NSData *report = [NSData dataWithBytes:result->report.data
                                    length:result->report.len];
    gf_finalcut_error_free(bridgeError);
    gf_finalcut_route_d_patch_result_free(result);
    return @{@"fcpxml" : fcpxml, @"report" : report};
}

+ (nullable NSDictionary<NSString *, NSData *> *)processBatchFCPXML:(NSData *)input
                                                        documentURL:(NSURL *)documentURL
                                                              error:(NSError **)error {
    NSArray<GFRouteDProjectSnapshot *> *snapshots =
        GFProjectSnapshotsFromFCPXML(input, documentURL);
    GFRouteDProjectInput *projectInputs = snapshots.count > 0
        ? calloc(snapshots.count, sizeof(GFRouteDProjectInput))
        : NULL;
    if (snapshots.count > 0 && projectInputs == NULL) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:NSPOSIXErrorDomain
                                         code:ENOMEM
                                     userInfo:nil];
        }
        return nil;
    }
    for (NSUInteger index = 0; index < snapshots.count; index++) {
        GFRouteDProjectSnapshot *snapshot = snapshots[index];
        projectInputs[index] = (GFRouteDProjectInput){
            .path_bytes = snapshot.pathData.bytes,
            .path_len = snapshot.pathData.length,
            .project_bytes = snapshot.projectData.bytes,
            .project_len = snapshot.projectData.length,
            .status = snapshot.status,
            .reserved = 0,
        };
    }
    GFRouteDPatchResult result = {0};
    GFError *bridgeError = NULL;
    GFStatus status = gf_finalcut_route_d_batch_patch_with_project_inputs(
        input.bytes,
        input.length,
        projectInputs,
        snapshots.count,
        &result,
        &bridgeError
    );
    free(projectInputs);
    return [self resultForStatus:status result:&result bridgeError:bridgeError error:error];
}

@end
