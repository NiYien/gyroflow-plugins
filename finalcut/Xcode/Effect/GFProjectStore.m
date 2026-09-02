#import "GFProjectStore.h"

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

static BOOL GFBuildProjectPayload(
    NSData *projectData,
    NSString * _Nullable *payload,
    NSError **error
) {
    GFError *bridgeError = NULL;
    GFFinalCutInstance *instance = gf_finalcut_instance_create(&bridgeError);
    if (instance == NULL) {
        NSString *message = GFProjectBridgeError(bridgeError, @"unable to create project validator");
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return NO;
    }
    GFStatus status = gf_finalcut_instance_load_project(
        instance,
        projectData.bytes,
        projectData.length,
        &bridgeError
    );
    if (status != GF_STATUS_OK) {
        NSString *message = GFProjectBridgeError(bridgeError, @"invalid Gyroflow project");
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidProject
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        gf_finalcut_instance_free(instance);
        return NO;
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
        NSString *message = GFProjectBridgeError(bridgeError, @"unable to encode project payload");
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidPayload
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return NO;
    }
    gf_finalcut_error_free(bridgeError);
    if (payload != NULL) {
        *payload = encodedPayload;
    }
    return YES;
}

@interface GFProjectStore ()
@property(nonatomic, copy) GFProjectPayloadBuilder payloadBuilder;
@property(nonatomic, readwrite, nullable) NSData *currentProjectData;
@property(nonatomic, readwrite, nullable) NSString *currentProjectPayload;
@property(nonatomic, readwrite) NSString *status;
@property(nonatomic, nullable) NSData *pendingPreviousProjectData;
@property(nonatomic, nullable) NSString *pendingPreviousProjectPayload;
@property(nonatomic, nullable) NSString *pendingPreviousStatus;
@property(nonatomic, nullable) NSString *pendingExpectedProjectPayload;
@property(nonatomic, nullable) NSString *pendingProjectName;
@end

@implementation GFProjectStore

- (instancetype)init {
    return [self initWithPayloadBuilder:^BOOL(
        NSData *projectData,
        NSString * _Nullable *payload,
        NSError **error
    ) {
        return GFBuildProjectPayload(projectData, payload, error);
    }];
}

- (instancetype)initWithPayloadBuilder:(GFProjectPayloadBuilder)payloadBuilder {
    self = [super init];
    if (self != nil) {
        self.payloadBuilder = payloadBuilder;
        self.status = @"Import a .gyroflow project";
    }
    return self;
}

- (BOOL)failWithCode:(GFProjectStoreErrorCode)code
                  url:(NSURL *)url
              message:(NSString *)message
                error:(NSError **)error {
    NSString *preservation = self.currentProjectPayload != nil
        ? @"kept previous project"
        : @"no project loaded";
    self.status = [NSString stringWithFormat:@"Rejected %@: %@; %@",
                   url.lastPathComponent,
                   message,
                   preservation];
    if (error != NULL) {
        *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                     code:code
                                 userInfo:@{NSLocalizedDescriptionKey : message}];
    }
    return NO;
}

- (BOOL)importProjectURL:(NSURL *)url error:(NSError **)error {
    return [self importProjectURL:url commitPayload:nil error:error];
}

- (BOOL)importProjectURL:(NSURL *)url
           commitPayload:(BOOL (^)(NSString *projectPayload))commitPayload
                   error:(NSError **)error {
    if (!url.isFileURL ||
        ![[url.pathExtension lowercaseString] isEqualToString:@"gyroflow"]) {
        return [self failWithCode:GFProjectStoreErrorWrongExtension
                              url:url
                          message:@"only .gyroflow project files are allowed"
                            error:error];
    }
    BOOL accessedSecurityScope = [url startAccessingSecurityScopedResource];
    NSError *readError = nil;
    NSData *data = [NSData dataWithContentsOfURL:url
                                        options:NSDataReadingMappedIfSafe
                                          error:&readError];
    if (accessedSecurityScope) {
        [url stopAccessingSecurityScopedResource];
    }
    if (data == nil) {
        return [self failWithCode:GFProjectStoreErrorReadFailed
                              url:url
                          message:[NSString stringWithFormat:@"project is not readable (%@)",
                                                             readError.localizedDescription
                                                                 ?: @"unknown error"]
                            error:error];
    }

    NSError *validationError = nil;
    NSString *payload = nil;
    if (!self.payloadBuilder(data, &payload, &validationError) || payload.length == 0) {
        return [self failWithCode:GFProjectStoreErrorInvalidProject
                              url:url
                          message:validationError.localizedDescription
                              ?: @"project validation failed"
                            error:error];
    }
    NSData *previousData = self.currentProjectData;
    NSString *previousPayload = self.currentProjectPayload;
    NSString *previousStatus = self.status;
    self.currentProjectData = [data copy];
    self.currentProjectPayload = [payload copy];
    self.status = [NSString stringWithFormat:@"Loaded %@ (%lu bytes)",
                   url.lastPathComponent,
                   (unsigned long)data.length];
    if (commitPayload != nil) {
        if (!commitPayload(payload)) {
            self.currentProjectData = previousData;
            self.currentProjectPayload = previousPayload;
            self.status = previousStatus;
            return [self failWithCode:GFProjectStoreErrorCommitFailed
                                  url:url
                              message:@"Final Cut did not persist the project payload"
                                error:error];
        }
        @synchronized(self) {
            self.pendingPreviousProjectData = previousData;
            self.pendingPreviousProjectPayload = previousPayload;
            self.pendingPreviousStatus = previousStatus;
            self.pendingExpectedProjectPayload = payload;
            self.pendingProjectName = url.lastPathComponent;
        }
    }
    return YES;
}

- (BOOL)reconcileHostPersistedProjectPayload:(NSString *)projectPayload {
    @synchronized(self) {
        if (self.pendingExpectedProjectPayload == nil) {
            return NO;
        }
        if (![projectPayload isEqualToString:self.pendingExpectedProjectPayload]) {
            self.currentProjectData = self.pendingPreviousProjectData;
            self.currentProjectPayload = self.pendingPreviousProjectPayload;
            NSString *preservation = self.currentProjectPayload != nil
                ? @"kept previous project"
                : @"no project loaded";
            self.status = [NSString stringWithFormat:
                @"Rejected %@: Final Cut did not persist the project payload; %@",
                self.pendingProjectName ?: @"project",
                preservation];
        }
        self.pendingPreviousProjectData = nil;
        self.pendingPreviousProjectPayload = nil;
        self.pendingPreviousStatus = nil;
        self.pendingExpectedProjectPayload = nil;
        self.pendingProjectName = nil;
        return YES;
    }
}

- (BOOL)restoreProjectPayload:(NSString *)payload error:(NSError **)error {
    if (payload.length == 0) {
        self.status = @"Import a .gyroflow project";
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
        NSString *message = GFProjectBridgeError(bridgeError, @"invalid embedded project payload");
        self.status = [NSString stringWithFormat:@"Embedded project unavailable: %@", message];
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFProjectStoreErrorDomain
                                         code:GFProjectStoreErrorInvalidPayload
                                     userInfo:@{NSLocalizedDescriptionKey : message}];
        }
        gf_finalcut_error_free(bridgeError);
        return NO;
    }
    gf_finalcut_error_free(bridgeError);
    self.currentProjectPayload = [payload copy];
    self.currentProjectData = nil;
    self.status = @"Embedded .gyroflow project ready";
    return YES;
}

- (BOOL)restorePersistedProjectPayload:(NSString *)projectPayload
                         timingPayload:(NSString *)timingPayload
                                 error:(NSError **)error {
    if (![self restoreProjectPayload:projectPayload error:error]) {
        return NO;
    }
    if (timingPayload.length == 0) {
        [self recordDirectModeReady];
    }
    return YES;
}

- (BOOL)restoreValidatedRenderProjectPayloadIfEmpty:(NSString *)projectPayload
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
        self.status = timingPayload.length == 0
            ? @"Direct mode ready (untrimmed forward 1×). Complex edits: use Process Current Final Cut Project."
            : @"Embedded .gyroflow project ready";
        return YES;
    }
}

- (void)recordAuthorizationCancellation {
    NSString *preservation = self.currentProjectPayload != nil
        ? @"kept previous project"
        : @"no project loaded";
    self.status = [NSString stringWithFormat:@"Authorization cancelled; %@", preservation];
}

- (void)recordDirectModeReady {
    self.status = @"Direct mode ready (untrimmed forward 1×). Complex edits: use Process Current Final Cut Project.";
}

- (void)recordReprocessRequired {
    self.status = self.currentProjectPayload != nil
        ? @"Project loaded; Reprocess Project Required for current timing"
        : @"Import a .gyroflow project";
}

@end
