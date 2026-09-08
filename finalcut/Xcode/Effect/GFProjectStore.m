#import "GFProjectStore.h"

#import "GFLocalization.h"
#import "GyroflowFinalCut.h"

NSString *const GFProjectStoreErrorDomain =
    @"com.niyien.gyroflow.finalcut.project-store";

static NSString *GFProjectBridgeError(const GFError *error, NSString *fallback) {
    if (error == NULL || error->message == NULL) {
        return fallback;
    }
    NSString *message = [NSString stringWithUTF8String:error->message];
    return message ?: fallback;
}

static NSString *GFStoredProjectName(NSString *displayName) {
    NSString *filename = displayName.lastPathComponent;
    if (filename.length == 0) {
        return GFLocalized(@"effect.project.embedded_name", @"Embedded project");
    }
    return filename;
}

static NSString *GFStoredProjectFilename(NSString *displayName) {
    NSString *filename = displayName.lastPathComponent;
    return filename.length > 0 ? filename : @"embedded-project";
}

static const NSUInteger GFMaximumRawProjectBytes = 256 * 1024 * 1024;
static const NSUInteger GFProjectReadChunkBytes = 1024 * 1024;
static const NSTimeInterval GFHostReadbackTimeoutSeconds = 5.0;

static NSData *GFReadBoundedProjectData(NSURL *url, NSError **error) {
    NSNumber *fileSize = nil;
    if (![url getResourceValue:&fileSize forKey:NSURLFileSizeKey error:error]) {
        return nil;
    }
    if (fileSize.unsignedLongLongValue > GFMaximumRawProjectBytes) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorReadFailed
                                     userInfo:@{
                                         NSLocalizedDescriptionKey : GFLocalized(
                                             @"effect.error.project_too_large",
                                             @"Project exceeds the 256 MiB size limit")
                                     }];
        }
        return nil;
    }
    NSFileHandle *handle = [NSFileHandle fileHandleForReadingFromURL:url error:error];
    if (handle == nil) {
        return nil;
    }
    NSMutableData *data = [NSMutableData dataWithCapacity:fileSize.unsignedIntegerValue];
    while (data.length <= GFMaximumRawProjectBytes) {
        NSUInteger remaining = GFMaximumRawProjectBytes + 1 - data.length;
        NSData *chunk = [handle readDataUpToLength:MIN(remaining, GFProjectReadChunkBytes)
                                            error:error];
        if (chunk == nil) {
            [handle closeAndReturnError:NULL];
            return nil;
        }
        if (chunk.length == 0) {
            break;
        }
        [data appendData:chunk];
    }
    [handle closeAndReturnError:NULL];
    if (data.length > GFMaximumRawProjectBytes) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorReadFailed
                                     userInfo:@{
                                         NSLocalizedDescriptionKey : GFLocalized(
                                             @"effect.error.project_too_large",
                                             @"Project exceeds the 256 MiB size limit")
                                     }];
        }
        return nil;
    }
    return data;
}

@interface GFProjectImportCandidate ()
@property(nonatomic, readwrite) NSData *projectData;
@property(nonatomic, readwrite) NSString *projectPayload;
@property(nonatomic, readwrite) NSString *projectDisplayName;
@property(nonatomic, readwrite) GFRenderParameters parameters;
@end

@implementation GFProjectImportCandidate

- (instancetype)initWithProjectData:(NSData *)projectData
                     projectPayload:(NSString *)projectPayload
                  projectDisplayName:(NSString *)projectDisplayName
                          parameters:(GFRenderParameters)parameters {
    self = [super init];
    if (self != nil) {
        self.projectData = [projectData copy];
        self.projectPayload = [projectPayload copy];
        self.projectDisplayName = [projectDisplayName copy];
        self.parameters = parameters;
    }
    return self;
}

@end

static GFProjectImportCandidate *GFBuildProjectImportCandidate(
    NSData *projectData,
    NSString *projectDisplayName,
    NSError **error
) {
    GFError *bridgeError = NULL;
    GFFinalCutInstance *instance = gf_finalcut_instance_create(&bridgeError);
    if (instance == NULL) {
        NSString *message = GFProjectBridgeError(
            bridgeError,
            GFLocalized(@"effect.error.validation_failed", @"Project validation failed"));
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return nil;
    }
    GFStatus status = gf_finalcut_instance_load_project(
        instance,
        projectData.bytes,
        projectData.length,
        &bridgeError
    );
    if (status != GF_STATUS_OK) {
        NSString *message = GFProjectBridgeError(
            bridgeError,
            GFLocalized(@"effect.error.validation_failed", @"Project validation failed"));
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        gf_finalcut_instance_free(instance);
        return nil;
    }
    gf_finalcut_error_free(bridgeError);
    bridgeError = NULL;

    GFRenderParameters parameters = {0};
    status = gf_finalcut_instance_get_project_render_parameters(
        instance,
        &parameters,
        &bridgeError
    );
    if (status != GF_STATUS_OK) {
        NSString *message = GFProjectBridgeError(
            bridgeError,
            GFLocalized(@"effect.error.validation_failed", @"Project validation failed")
        );
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        gf_finalcut_instance_free(instance);
        return nil;
    }
    gf_finalcut_error_free(bridgeError);
    bridgeError = NULL;

    GFOwnedBytes encoded = {0};
    status = gf_finalcut_project_payload_encode(
        projectData.bytes,
        projectData.length,
        &encoded,
        &bridgeError
    );
    NSString *encodedPayload = nil;
    if (status == GF_STATUS_OK) {
        encodedPayload = [[NSString alloc] initWithBytes:encoded.data
                                                 length:encoded.len
                                               encoding:NSASCIIStringEncoding];
    }
    gf_finalcut_owned_bytes_free(&encoded);
    gf_finalcut_instance_free(instance);
    if (status != GF_STATUS_OK || encodedPayload.length == 0) {
        NSString *message = GFProjectBridgeError(
            bridgeError,
            GFLocalized(@"effect.error.validation_failed", @"Project validation failed"));
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidPayload
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return nil;
    }
    gf_finalcut_error_free(bridgeError);
    return [[GFProjectImportCandidate alloc]
        initWithProjectData:projectData
             projectPayload:encodedPayload
          projectDisplayName:projectDisplayName
                  parameters:parameters];
}

@interface GFProjectStore ()
@property(nonatomic, copy) GFProjectImportCandidateBuilder importCandidateBuilder;
@property(nonatomic, readwrite, nullable) NSData *currentProjectData;
@property(nonatomic, readwrite, nullable) NSString *currentProjectPayload;
@property(nonatomic, readwrite, nullable) GFProjectImportCandidate *currentProjectCandidate;
@property(nonatomic, readwrite) NSString *currentProjectName;
@property(nonatomic, readwrite) NSString *currentProjectFilename;
@property(nonatomic, readwrite) NSString *status;
@property(nonatomic, readwrite) GFProjectModeStatus modeStatus;
@property(nonatomic, readwrite) NSUInteger currentProjectGeneration;
@property(nonatomic) NSUInteger nextProjectGeneration;
@property(nonatomic) BOOL pendingCommitInFlight;
@property(nonatomic, nullable) NSData *pendingPreviousProjectData;
@property(nonatomic, nullable) NSString *pendingPreviousProjectPayload;
@property(nonatomic, nullable) GFProjectImportCandidate *pendingPreviousProjectCandidate;
@property(nonatomic, nullable) NSString *pendingPreviousProjectName;
@property(nonatomic, nullable) NSString *pendingPreviousProjectFilename;
@property(nonatomic, nullable) NSString *pendingPreviousStatus;
@property(nonatomic) GFProjectModeStatus pendingPreviousModeStatus;
@property(nonatomic, nullable) NSString *pendingExpectedProjectPayload;
@property(nonatomic, nullable) GFProjectImportCandidate *pendingProjectCandidate;
@property(nonatomic, nullable) NSString *pendingProjectName;
@property(nonatomic) NSUInteger pendingPreviousProjectGeneration;
@property(nonatomic) NSUInteger pendingProjectGeneration;
@end

@implementation GFProjectStore

- (instancetype)init {
    return [self initWithImportCandidateBuilder:^GFProjectImportCandidate *(
        NSData *projectData,
        NSString *projectDisplayName,
        NSError **error
    ) {
        return GFBuildProjectImportCandidate(projectData, projectDisplayName, error);
    }];
}

- (instancetype)initWithImportCandidateBuilder:
        (GFProjectImportCandidateBuilder)importCandidateBuilder {
    self = [super init];
    if (self != nil) {
        self.importCandidateBuilder = importCandidateBuilder;
        self.currentProjectName = @"";
        self.currentProjectFilename = @"embedded-project";
        self.status = GFLocalized(@"effect.status.load_project",
                                  @"Load a .gyroflow project");
        self.modeStatus = GFProjectModeStatusNone;
        self.nextProjectGeneration = 1;
    }
    return self;
}

- (BOOL)failWithCode:(GFProjectStoreErrorCode)code
                  url:(NSURL *)url
                message:(NSString *)message
                error:(NSError **)error {
    @synchronized(self) {
        NSString *preservation = self.currentProjectPayload != nil
            ? GFLocalized(@"effect.status.kept_previous", @"kept previous project")
            : GFLocalized(@"effect.status.none_loaded", @"no project loaded");
        self.status = [NSString stringWithFormat:GFLocalized(
                           @"effect.status.rejected", @"Rejected %@: %@; %@"),
                       url.lastPathComponent,
                       message,
                       preservation];
    }
    if (error != NULL) {
        *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                     code:code
                                 userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return NO;
}

- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error {
    return [self importProjectURL:url commitCandidate:nil error:error];
}

- (nullable GFProjectImportCandidate *)prepareImportProjectURL:(NSURL *)url
                                                          error:(NSError **)error {
    if (!url.isFileURL ||
        ![[url.pathExtension lowercaseString] isEqualToString:@"gyroflow"]) {
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorWrongExtension
                                     userInfo:@{
                                         NSLocalizedDescriptionKey : GFLocalized(
                                             @"effect.error.only_gyroflow",
                                             @"Only .gyroflow project files are allowed")
                                     }];
        }
        return nil;
    }
    BOOL accessedSecurityScope = [url startAccessingSecurityScopedResource];
    NSError *readError = nil;
    NSData *data = GFReadBoundedProjectData(url, &readError);
    if (accessedSecurityScope) {
        [url stopAccessingSecurityScopedResource];
    }
    if (data == nil) {
        if (error != NULL) {
            NSString *message = [NSString stringWithFormat:GFLocalized(
                                     @"effect.error.project_unreadable",
                                     @"Project is not readable (%@)"),
                                 readError.localizedDescription
                                     ?: GFLocalized(@"effect.error.unknown",
                                                    @"unknown error")];
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorReadFailed
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        return nil;
    }

    NSString *projectDisplayName = url.lastPathComponent;
    NSError *validationError = nil;
    GFProjectImportCandidate *builtCandidate =
        self.importCandidateBuilder(data, projectDisplayName, &validationError);
    if (builtCandidate == nil || builtCandidate.projectPayload.length == 0) {
        if (error != NULL) {
            NSString *message = validationError.localizedDescription
                ?: GFLocalized(@"effect.error.validation_failed",
                               @"Project validation failed");
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        return nil;
    }
    return [[GFProjectImportCandidate alloc]
        initWithProjectData:data
             projectPayload:builtCandidate.projectPayload
          projectDisplayName:projectDisplayName
                  parameters:builtCandidate.parameters];
}

- (void)recordImportFailureForURL:(NSURL *)url error:(NSError *)error {
    GFProjectStoreErrorCode code = [error.domain isEqualToString:GFProjectStoreErrorDomain]
        ? (GFProjectStoreErrorCode)error.code
        : GFProjectStoreErrorInvalidProject;
    [self failWithCode:code
                   url:url
               message:error.localizedDescription
                   ?: GFLocalized(@"effect.error.validation_failed",
                                  @"Project validation failed")
                 error:NULL];
}

- (BOOL)commitPreparedImportCandidate:(GFProjectImportCandidate *)candidate
                            sourceURL:(NSURL *)sourceURL
                       commitCandidate:(BOOL (^)(GFProjectImportCandidate *candidate))commitCandidate
                                 error:(NSError **)error {
    __block NSData *previousData = nil;
    __block NSString *previousPayload = nil;
    __block GFProjectImportCandidate *previousCandidate = nil;
    __block NSString *previousProjectName = nil;
    __block NSString *previousProjectFilename = nil;
    __block NSString *previousStatus = nil;
    __block GFProjectModeStatus previousModeStatus = GFProjectModeStatusNone;
    __block NSUInteger previousGeneration = 0;
    __block NSUInteger pendingGeneration = 0;
    if (commitCandidate != nil) {
        __block BOOL reserved = NO;
        @synchronized(self) {
            if (!self.pendingCommitInFlight && self.pendingExpectedProjectPayload == nil) {
                self.pendingCommitInFlight = YES;
                previousData = self.currentProjectData;
                previousPayload = self.currentProjectPayload;
                previousCandidate = self.currentProjectCandidate;
                previousProjectName = self.currentProjectName;
                previousProjectFilename = self.currentProjectFilename;
                previousStatus = self.status;
                previousModeStatus = self.modeStatus;
                previousGeneration = self.currentProjectGeneration;
                reserved = YES;
            }
        }
        if (!reserved) {
            return [self failWithCode:GFProjectStoreErrorCommitPending
                                  url:sourceURL
                              message:GFLocalized(
                                  @"effect.error.commit_pending",
                                  @"Wait for Final Cut to confirm the previous project load")
                                error:error];
        }
        // Commit the new candidate without publishing it as the current project first.
        BOOL committed = commitCandidate(candidate);
        @synchronized(self) {
            self.pendingCommitInFlight = NO;
        }
        if (!committed) {
            return [self failWithCode:GFProjectStoreErrorCommitFailed
                                  url:sourceURL
                              message:GFLocalized(@"effect.error.persist_failed",
                                                  @"Final Cut did not persist the project payload")
                                error:error];
        }
        @synchronized(self) {
            pendingGeneration = self.nextProjectGeneration++;
            self.pendingPreviousProjectData = previousData;
            self.pendingPreviousProjectPayload = previousPayload;
            self.pendingPreviousProjectCandidate = previousCandidate;
            self.pendingPreviousProjectName = previousProjectName;
            self.pendingPreviousProjectFilename = previousProjectFilename;
            self.pendingPreviousStatus = previousStatus;
            self.pendingPreviousModeStatus = previousModeStatus;
            self.pendingExpectedProjectPayload = candidate.projectPayload;
            self.pendingProjectCandidate = candidate;
            self.pendingProjectName = candidate.projectDisplayName;
            self.pendingPreviousProjectGeneration = previousGeneration;
            self.pendingProjectGeneration = pendingGeneration;
            self.status = [NSString stringWithFormat:GFLocalized(
                               @"effect.status.awaiting_confirmation",
                               @"Waiting for Final Cut to confirm %@"),
                           candidate.projectDisplayName];
            self.modeStatus = GFProjectModeStatusPending;
        }
        dispatch_after(dispatch_time(
                           DISPATCH_TIME_NOW,
                           (int64_t)(GFHostReadbackTimeoutSeconds * NSEC_PER_SEC)),
                       dispatch_get_global_queue(QOS_CLASS_UTILITY, 0), ^{
            [self expirePendingHostReadbackGeneration:pendingGeneration];
        });
    } else {
        @synchronized(self) {
            self.currentProjectData = candidate.projectData;
            self.currentProjectPayload = candidate.projectPayload;
            self.currentProjectCandidate = candidate;
            self.currentProjectName = candidate.projectDisplayName;
            self.currentProjectFilename = candidate.projectDisplayName;
            self.currentProjectGeneration = self.nextProjectGeneration++;
            self.status = [NSString stringWithFormat:GFLocalized(
                               @"effect.status.loaded", @"Loaded %@ (%lu bytes)"),
                           self.currentProjectName,
                           (unsigned long)candidate.projectData.length];
            self.modeStatus = GFProjectModeStatusDirect;
        }
    }
    return YES;
}

- (BOOL)importProjectURL:(NSURL *)url
         commitCandidate:(BOOL (^)(GFProjectImportCandidate *candidate))commitCandidate
                   error:(NSError **)error {
    NSError *preparationError = nil;
    GFProjectImportCandidate *candidate = [self prepareImportProjectURL:url
                                                                   error:&preparationError];
    if (candidate == nil) {
        NSError *resolvedError = preparationError
            ?: [NSError errorWithDomain:GFProjectStoreErrorDomain
                                    code:GFProjectStoreErrorInvalidProject
                                userInfo:@{
                                    NSLocalizedDescriptionKey : GFLocalized(
                                        @"effect.error.validation_failed",
                                        @"Project validation failed")
                                }];
        [self recordImportFailureForURL:url error:resolvedError];
        if (error != NULL) {
            *error = resolvedError;
        }
        return NO;
    }
    return [self commitPreparedImportCandidate:candidate
                                     sourceURL:url
                                commitCandidate:commitCandidate
                                          error:error];
}

- (NSUInteger)beginHostProjectReadback {
    @synchronized(self) {
        return self.pendingExpectedProjectPayload != nil
            ? self.pendingProjectGeneration
            : self.currentProjectGeneration;
    }
}

- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload {
    return [self reconcileHostPersistedProjectPayload:projectPayload
                                   readbackGeneration:[self beginHostProjectReadback]];
}

- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload
                          readbackGeneration:(NSUInteger)readbackGeneration {
    @synchronized(self) {
        if (self.pendingExpectedProjectPayload == nil ||
            readbackGeneration != self.pendingProjectGeneration) {
            return NO;
        }
        if (![projectPayload isEqualToString:self.pendingExpectedProjectPayload]) {
            NSString *preservation = self.currentProjectPayload != nil
                ? GFLocalized(@"effect.status.kept_previous", @"kept previous project")
                : GFLocalized(@"effect.status.none_loaded", @"no project loaded");
            NSString *failure = GFLocalized(@"effect.error.persist_failed",
                @"Final Cut did not persist the project payload");
            self.status = [NSString stringWithFormat:GFLocalized(
                @"effect.status.rejected", @"Rejected %@: %@; %@"),
                self.pendingProjectName
                    ?: GFLocalized(@"effect.project.unnamed", @"project"),
                failure,
                preservation];
            self.modeStatus = self.pendingPreviousModeStatus;
        } else {
            GFProjectImportCandidate *candidate = self.pendingProjectCandidate;
            self.currentProjectData = candidate.projectData;
            self.currentProjectPayload = candidate.projectPayload;
            self.currentProjectCandidate = candidate;
            self.currentProjectName = candidate.projectDisplayName;
            self.currentProjectFilename = candidate.projectDisplayName;
            self.currentProjectGeneration = self.pendingProjectGeneration;
            self.status = [NSString stringWithFormat:GFLocalized(
                               @"effect.status.loaded", @"Loaded %@ (%lu bytes)"),
                           self.currentProjectName,
                           (unsigned long)candidate.projectData.length];
            self.modeStatus = GFProjectModeStatusDirect;
        }
        self.pendingPreviousProjectData = nil;
        self.pendingPreviousProjectPayload = nil;
        self.pendingPreviousProjectCandidate = nil;
        self.pendingPreviousProjectName = nil;
        self.pendingPreviousProjectFilename = nil;
        self.pendingPreviousStatus = nil;
        self.pendingPreviousModeStatus = GFProjectModeStatusNone;
        self.pendingExpectedProjectPayload = nil;
        self.pendingProjectCandidate = nil;
        self.pendingProjectName = nil;
        self.pendingPreviousProjectGeneration = 0;
        self.pendingProjectGeneration = 0;
        return YES;
    }
}

- (BOOL)failPendingHostReadbackLockedWithMessage:(NSString *)message {
    if (self.pendingExpectedProjectPayload == nil) {
        return NO;
    }
    NSString *preservation = self.currentProjectPayload != nil
        ? GFLocalized(@"effect.status.kept_previous", @"kept previous project")
        : GFLocalized(@"effect.status.none_loaded", @"no project loaded");
    self.status = [NSString stringWithFormat:GFLocalized(
                       @"effect.status.rejected", @"Rejected %@: %@; %@"),
                   self.pendingProjectName
                       ?: GFLocalized(@"effect.project.unnamed", @"project"),
                   message,
                   preservation];
    self.modeStatus = self.pendingPreviousModeStatus;
    self.pendingPreviousProjectData = nil;
    self.pendingPreviousProjectPayload = nil;
    self.pendingPreviousProjectCandidate = nil;
    self.pendingPreviousProjectName = nil;
    self.pendingPreviousProjectFilename = nil;
    self.pendingPreviousStatus = nil;
    self.pendingPreviousModeStatus = GFProjectModeStatusNone;
    self.pendingExpectedProjectPayload = nil;
    self.pendingProjectCandidate = nil;
    self.pendingProjectName = nil;
    self.pendingPreviousProjectGeneration = 0;
    self.pendingProjectGeneration = 0;
    return YES;
}

- (BOOL)failPendingHostReadbackWithMessage:(NSString *)message {
    @synchronized(self) {
        return [self failPendingHostReadbackLockedWithMessage:message];
    }
}

- (BOOL)expirePendingHostReadbackGeneration:(NSUInteger)generation {
    @synchronized(self) {
        if (self.pendingExpectedProjectPayload == nil ||
            generation != self.pendingProjectGeneration) {
            return NO;
        }
        return [self failPendingHostReadbackLockedWithMessage:GFLocalized(
            @"effect.error.persist_timeout",
            @"Final Cut did not confirm the project payload in time")];
    }
}

- (BOOL)restoreProjectPayload:(NSString *)payload error:(NSError **)error {
    return [self restoreProjectPayload:payload displayName:@"" error:error];
}

- (BOOL)restoreProjectPayload:(NSString *)payload
                  displayName:(NSString *)displayName
                        error:(NSError **)error {
    if (payload.length == 0) {
        @synchronized(self) {
            self.status = GFLocalized(@"effect.status.load_project",
                                      @"Load a .gyroflow project");
            self.modeStatus = GFProjectModeStatusNone;
        }
        return NO;
    }
    NSData *payloadData = [payload dataUsingEncoding:NSASCIIStringEncoding];
    GFError *bridgeError = NULL;
    GFFinalCutInstance *instance = gf_finalcut_instance_create(&bridgeError);
    GFStatus status = instance == NULL
        ? GF_STATUS_PANIC
        : gf_finalcut_instance_load_project_payload(
              instance,
              payloadData.bytes,
              payloadData.length,
              &bridgeError
          );
    gf_finalcut_instance_free(instance);
    if (status != GF_STATUS_OK) {
        NSString *message = GFProjectBridgeError(
            bridgeError,
            GFLocalized(@"effect.error.validation_failed", @"Project validation failed"));
        @synchronized(self) {
            self.status = [NSString stringWithFormat:GFLocalized(
                               @"effect.status.embedded_unavailable",
                               @"Embedded project unavailable: %@"),
                           message];
        }
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidPayload
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return NO;
    }
    gf_finalcut_error_free(bridgeError);
    @synchronized(self) {
        self.currentProjectPayload = [payload copy];
        self.currentProjectData = nil;
        self.currentProjectCandidate = nil;
        self.currentProjectName = GFStoredProjectName(displayName);
        self.currentProjectFilename = GFStoredProjectFilename(displayName);
        self.currentProjectGeneration = self.nextProjectGeneration++;
        self.status = [NSString stringWithFormat:GFLocalized(
                           @"effect.status.ready", @"%@ ready"),
                       self.currentProjectName];
        self.modeStatus = GFProjectModeStatusDirect;
    }
    return YES;
}

- (BOOL)restorePersistedProjectPayload:(NSString *)projectPayload
                           displayName:(NSString *)displayName
                         timingPayload:(NSString *)timingPayload
                                 error:(NSError **)error {
    if (![self restoreProjectPayload:projectPayload
                         displayName:displayName
                               error:error]) {
        return NO;
    }
    if (timingPayload.length == 0) {
        [self recordDirectModeReady];
    } else {
        [self recordRouteDModeReady];
    }
    return YES;
}

- (BOOL)restoreValidatedRenderProjectPayloadIfEmpty:(NSString *)projectPayload
                                        displayName:(NSString *)displayName
                                      timingPayload:(NSString *)timingPayload {
    if (projectPayload.length == 0) {
        return NO;
    }
    @synchronized(self) {
        if (self.currentProjectPayload != nil) {
            return NO;
        }
        self.currentProjectPayload = [projectPayload copy];
        self.currentProjectData = nil;
        self.currentProjectCandidate = nil;
        self.currentProjectName = GFStoredProjectName(displayName);
        self.currentProjectFilename = GFStoredProjectFilename(displayName);
        self.currentProjectGeneration = self.nextProjectGeneration++;
        self.status = timingPayload.length == 0
            ? [NSString stringWithFormat:GFLocalized(
                  @"effect.status.ready_direct", @"%@ ready (direct mode)"),
                  self.currentProjectName]
            : [NSString stringWithFormat:GFLocalized(
                  @"effect.status.ready", @"%@ ready"), self.currentProjectName];
        self.modeStatus = timingPayload.length == 0
            ? GFProjectModeStatusDirect
            : GFProjectModeStatusRouteD;
        return YES;
    }
}

- (void)recordAuthorizationCancellation {
    @synchronized(self) {
        NSString *preservation = self.currentProjectPayload != nil
            ? GFLocalized(@"effect.status.kept_previous", @"kept previous project")
            : GFLocalized(@"effect.status.none_loaded", @"no project loaded");
        self.status = [NSString stringWithFormat:GFLocalized(
                           @"effect.status.authorization_cancelled",
                           @"Authorization cancelled; %@"),
                       preservation];
    }
}

- (void)recordDirectModeReady {
    @synchronized(self) {
        NSString *name = self.currentProjectName.length > 0
            ? self.currentProjectName
            : GFLocalized(@"effect.project.embedded_name", @"Embedded project");
        self.status = [NSString stringWithFormat:GFLocalized(
                           @"effect.status.ready_direct", @"%@ ready (direct mode)"),
                       name];
        self.modeStatus = GFProjectModeStatusDirect;
    }
}

- (void)recordRouteDModeReady {
    @synchronized(self) {
        NSString *name = self.currentProjectName.length > 0
            ? self.currentProjectName
            : GFLocalized(@"effect.project.embedded_name", @"Embedded project");
        self.status = [NSString stringWithFormat:GFLocalized(
                           @"effect.status.ready_route_d", @"%@ ready (Route D)"),
                       name];
        self.modeStatus = GFProjectModeStatusRouteD;
    }
}

- (void)recordReprocessRequired {
    @synchronized(self) {
        self.status = self.currentProjectPayload != nil
            ? [NSString stringWithFormat:GFLocalized(
                  @"effect.status.reprocess_required",
                  @"%@; Reprocess Project Required for current timing"),
                  self.currentProjectName]
            : GFLocalized(@"effect.status.load_project", @"Load a .gyroflow project");
        self.modeStatus = self.currentProjectPayload != nil
            ? GFProjectModeStatusReprocessRequired
            : GFProjectModeStatusNone;
    }
}

@end
