#import <Foundation/Foundation.h>
#import <string.h>

#import "GFProjectStore.h"
#import "GyroflowFinalCut.h"

static BOOL GFAllowPayloadRestore = NO;

GFFinalCutInstance *gf_finalcut_instance_create(GFError **out_error) {
    (void)out_error;
    return GFAllowPayloadRestore ? (GFFinalCutInstance *)0x1 : NULL;
}
void gf_finalcut_instance_free(GFFinalCutInstance *instance) {
    (void)instance;
}
GFStatus gf_finalcut_instance_load_project(
    GFFinalCutInstance *instance,
    const uint8_t *project_bytes,
    size_t project_len,
    GFError **out_error
) {
    (void)instance;
    (void)project_bytes;
    (void)project_len;
    (void)out_error;
    return GF_STATUS_INVALID_PROJECT;
}
GFStatus gf_finalcut_project_payload_encode(
    const uint8_t *project_bytes,
    size_t project_len,
    GFOwnedBytes *out_payload,
    GFError **out_error
) {
    (void)project_bytes;
    (void)project_len;
    (void)out_payload;
    (void)out_error;
    return GF_STATUS_INVALID_PROJECT;
}
GFStatus gf_finalcut_instance_get_project_render_parameters(
    const GFFinalCutInstance *instance,
    GFRenderParameters *out_parameters,
    GFError **out_error
) {
    (void)instance;
    (void)out_parameters;
    (void)out_error;
    return GF_STATUS_INVALID_PROJECT;
}
GFStatus gf_finalcut_instance_load_project_payload(
    GFFinalCutInstance *instance,
    const uint8_t *payload_bytes,
    size_t payload_len,
    GFError **out_error
) {
    (void)instance;
    (void)out_error;
    return GFAllowPayloadRestore && payload_len == strlen("persisted-payload") &&
            memcmp(payload_bytes, "persisted-payload", payload_len) == 0
        ? GF_STATUS_OK
        : GF_STATUS_INVALID_PROJECT;
}
void gf_finalcut_error_free(GFError *error) {
    (void)error;
}
void gf_finalcut_owned_bytes_free(GFOwnedBytes *bytes) {
    (void)bytes;
}

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        __block NSString *builderDisplayName = @"";
        __block NSUInteger builderCalls = 0;
        GFProjectStore *store = [[GFProjectStore alloc]
            initWithImportCandidateBuilder:^GFProjectImportCandidate *(
                NSData *projectData,
                NSString *projectDisplayName,
                NSError **error
            ) {
                builderCalls += 1;
                builderDisplayName = [projectDisplayName copy];
                NSString *text = [[NSString alloc] initWithData:projectData
                                                       encoding:NSUTF8StringEncoding];
                if ([text containsString:@"invalid"]) {
                    if (error != NULL) {
                        *error = [NSError errorWithDomain:@"test"
                                                     code:1
                                                 userInfo:@{
                                                     NSLocalizedDescriptionKey :
                                                         @"core rejected project"
                                                 }];
                    }
                    return nil;
                }
                GFRenderParameters parameters = {
                    .fov = 1.25,
                    .smoothness = 42.0,
                    .lens_correction = 95.0,
                    .horizon_lock_amount = 33.0,
                    .horizon_lock_roll = 4.0,
                    .zoom_mode = 0,
                    .overview = 1,
                    .reserved = {0, 0, 0},
                };
                return [[GFProjectImportCandidate alloc]
                    initWithProjectData:projectData
                         projectPayload:[NSString stringWithFormat:@"payload-%lu",
                                                                  (unsigned long)projectData.length]
                      projectDisplayName:projectDisplayName
                              parameters:parameters];
            }];
        NSMutableArray<NSString *> *errors = [NSMutableArray array];
        BOOL restoreSucceeded = NO;
        BOOL hostReconciled = NO;
        BOOL secondPendingLoadAccepted = NO;
        __block NSUInteger pendingHostCommitCalls = 0;
        BOOL pendingGenerationReconciled = NO;
        BOOL generationReadbackAPIAvailable = NO;
        BOOL staleHostReadbackApplied = NO;
        BOOL staleHostReadbackPreservedCurrentProject = NO;
        BOOL freshHostReadbackReconciled = NO;
        BOOL renderRestoreSucceeded = NO;
        BOOL renderRestoreReplaced = NO;
        BOOL readbackFailureClearedPending = NO;
        BOOL readbackFailurePreservedPrevious = NO;
        BOOL retryAfterReadbackFailureSucceeded = NO;
        BOOL wrongGenerationTimeoutIgnored = NO;
        BOOL matchingGenerationTimeoutApplied = NO;
        if (argc == 4 && strcmp(argv[1], "--host-reject") == 0) {
            NSURL *previousURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *replacementURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            [store importProjectURL:previousURL error:NULL];
            NSString *previousPayload = store.currentProjectPayload;
            [store importProjectURL:replacementURL
                    commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                          (void)candidate;
                          return YES;
                      }
                              error:NULL];
            hostReconciled =
                [store reconcileHostPersistedProjectPayload:previousPayload ?: @""];
        }
        if (argc == 4 && strcmp(argv[1], "--pending-load") == 0) {
            NSURL *firstURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *secondURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            __block NSString *firstPayload = nil;
            [store importProjectURL:firstURL
                    commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                          firstPayload = candidate.projectPayload;
                          pendingHostCommitCalls += 1;
                          return YES;
                      }
                              error:NULL];
            secondPendingLoadAccepted = [store
                importProjectURL:secondURL
                  commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                        (void)candidate;
                        pendingHostCommitCalls += 1;
                        return YES;
                    }
                            error:NULL];
            pendingGenerationReconciled =
                [store reconcileHostPersistedProjectPayload:firstPayload ?: @""];
        }
        if (argc == 4 && strcmp(argv[1], "--stale-host-readback") == 0) {
            NSURL *firstURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *secondURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            SEL beginSelector = NSSelectorFromString(@"beginHostProjectReadback");
            SEL reconcileSelector = NSSelectorFromString(
                @"reconcileHostPersistedProjectPayload:readbackGeneration:");
            generationReadbackAPIAvailable =
                [store respondsToSelector:beginSelector] &&
                [store respondsToSelector:reconcileSelector];
            __block NSString *firstPayload = nil;
            [store importProjectURL:firstURL
                    commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                          firstPayload = candidate.projectPayload;
                          pendingHostCommitCalls += 1;
                          return YES;
                      }
                              error:NULL];
            NSUInteger firstReadbackGeneration = 0;
            if (generationReadbackAPIAvailable) {
                NSUInteger (*beginReadback)(id, SEL) =
                    (NSUInteger (*)(id, SEL))[store methodForSelector:beginSelector];
                BOOL (*reconcileReadback)(id, SEL, NSString *, NSUInteger) =
                    (BOOL (*)(id, SEL, NSString *, NSUInteger))
                        [store methodForSelector:reconcileSelector];
                firstReadbackGeneration = beginReadback(store, beginSelector);
                pendingGenerationReconciled = reconcileReadback(
                    store,
                    reconcileSelector,
                    firstPayload ?: @"",
                    firstReadbackGeneration
                );
                __block NSString *secondPayload = nil;
                [store importProjectURL:secondURL
                        commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                              secondPayload = candidate.projectPayload;
                              pendingHostCommitCalls += 1;
                              return YES;
                          }
                                  error:NULL];
                staleHostReadbackApplied = reconcileReadback(
                    store,
                    reconcileSelector,
                    firstPayload ?: @"",
                    firstReadbackGeneration
                );
                staleHostReadbackPreservedCurrentProject =
                    [store.currentProjectPayload isEqualToString:firstPayload] &&
                    [store.currentProjectName isEqualToString:@"A.gyroflow"];
                NSUInteger secondReadbackGeneration = beginReadback(store, beginSelector);
                freshHostReadbackReconciled = reconcileReadback(
                    store,
                    reconcileSelector,
                    secondPayload ?: @"",
                    secondReadbackGeneration
                ) && [store.currentProjectName isEqualToString:@"B.gyroflow"];
            }
        }
        if (argc == 4 && strcmp(argv[1], "--readback-failure") == 0) {
            NSURL *previousURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *replacementURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            [store importProjectURL:previousURL error:NULL];
            NSString *previousPayload = store.currentProjectPayload;
            __block NSString *pendingPayload = nil;
            [store importProjectURL:replacementURL
                    commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                          pendingPayload = candidate.projectPayload;
                          return YES;
                      }
                              error:NULL];
            readbackFailureClearedPending = [store
                failPendingHostReadbackWithMessage:@"readback unavailable"];
            readbackFailurePreservedPrevious =
                [store.currentProjectPayload isEqualToString:previousPayload] &&
                [store.currentProjectName isEqualToString:@"A.gyroflow"];
            __block NSString *retryPayload = nil;
            retryAfterReadbackFailureSucceeded = [store
                importProjectURL:replacementURL
                  commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                        retryPayload = candidate.projectPayload;
                        return YES;
                    }
                            error:NULL];
            retryAfterReadbackFailureSucceeded = retryAfterReadbackFailureSucceeded &&
                [store reconcileHostPersistedProjectPayload:retryPayload ?: @""] &&
                [store.currentProjectName isEqualToString:@"B.gyroflow"] &&
                pendingPayload.length > 0;
        }
        if (argc == 4 && strcmp(argv[1], "--pending-timeout") == 0) {
            NSURL *previousURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *replacementURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            [store importProjectURL:previousURL error:NULL];
            [store importProjectURL:replacementURL
                    commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                          (void)candidate;
                          return YES;
                      }
                              error:NULL];
            NSUInteger pendingGeneration = [store beginHostProjectReadback];
            wrongGenerationTimeoutIgnored = ![store
                expirePendingHostReadbackGeneration:pendingGeneration + 1];
            matchingGenerationTimeoutApplied = [store
                expirePendingHostReadbackGeneration:pendingGeneration];
            readbackFailurePreservedPrevious =
                [store.currentProjectName isEqualToString:@"A.gyroflow"];
            retryAfterReadbackFailureSucceeded = [store
                importProjectURL:replacementURL
                  commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                        (void)candidate;
                        return YES;
                    }
                            error:NULL];
        }
        BOOL rejectNextCommit = NO;
        BOOL specialMode = argc == 4 &&
            (strcmp(argv[1], "--host-reject") == 0 ||
             strcmp(argv[1], "--pending-load") == 0 ||
             strcmp(argv[1], "--stale-host-readback") == 0 ||
             strcmp(argv[1], "--readback-failure") == 0 ||
             strcmp(argv[1], "--pending-timeout") == 0);
        for (int index = specialMode ? argc : 1;
             index < argc;
             index++) {
            NSString *argument = [NSString stringWithUTF8String:argv[index]];
            if ([argument isEqualToString:@"--restore-no-timing"]) {
                GFAllowPayloadRestore = YES;
                NSError *error = nil;
                restoreSucceeded = [store
                    restorePersistedProjectPayload:@"persisted-payload"
                                       displayName:@"P1004783.gyroflow"
                                     timingPayload:@""
                                             error:&error];
                if (!restoreSucceeded) {
                    [errors addObject:error.localizedDescription ?: @"unknown"];
                }
                continue;
            }
            if ([argument isEqualToString:@"--restore-validated-render-state"]) {
                renderRestoreSucceeded = [store
                    restoreValidatedRenderProjectPayloadIfEmpty:@"persisted-payload"
                                                    displayName:@"P1004783.gyroflow"
                                                  timingPayload:@""];
                renderRestoreReplaced = [store
                    restoreValidatedRenderProjectPayloadIfEmpty:@"replacement-payload"
                                                    displayName:@"replacement.gyroflow"
                                                  timingPayload:@"timing"];
                continue;
            }
            if ([argument isEqualToString:@"--restore-no-name"]) {
                GFAllowPayloadRestore = YES;
                NSError *error = nil;
                restoreSucceeded = [store
                    restorePersistedProjectPayload:@"persisted-payload"
                                       displayName:@""
                                     timingPayload:@"timing"
                                             error:&error];
                if (!restoreSucceeded) {
                    [errors addObject:error.localizedDescription ?: @"unknown"];
                }
                continue;
            }
            if ([argument isEqualToString:@"--cancel"]) {
                [store recordAuthorizationCancellation];
                continue;
            }
            if ([argument isEqualToString:@"--reject-commit"]) {
                rejectNextCommit = YES;
                continue;
            }
            NSError *error = nil;
            BOOL imported = rejectNextCommit
                ? [store importProjectURL:[NSURL fileURLWithPath:argument]
                          commitCandidate:^BOOL(GFProjectImportCandidate *candidate) {
                                (void)candidate;
                                return NO;
                            }
                                    error:&error]
                : [store importProjectURL:[NSURL fileURLWithPath:argument] error:&error];
            rejectNextCommit = NO;
            if (!imported) {
                [errors addObject:error.localizedDescription ?: @"unknown"];
            }
        }
        NSDictionary *result = @{
            @"currentBytes" : @(store.currentProjectData.length),
            @"currentPayload" : store.currentProjectPayload ?: @"",
            @"currentProjectName" : store.currentProjectName,
            @"currentProjectFilename" : store.currentProjectFilename,
            @"builderDisplayName" : builderDisplayName,
            @"builderCalls" : @(builderCalls),
            @"candidateFOV" : @(store.currentProjectCandidate.parameters.fov),
            @"candidateSmoothness" : @(store.currentProjectCandidate.parameters.smoothness),
            @"candidateLensCorrection" : @(store.currentProjectCandidate.parameters.lens_correction),
            @"candidateHorizonLock" : @(store.currentProjectCandidate.parameters.horizon_lock_amount),
            @"candidateHorizonRoll" : @(store.currentProjectCandidate.parameters.horizon_lock_roll),
            @"candidateZoomMode" : @(store.currentProjectCandidate.parameters.zoom_mode),
            @"candidateOverview" : @(store.currentProjectCandidate.parameters.overview),
            @"status" : store.status,
            @"errors" : errors,
            @"restoreSucceeded" : @(restoreSucceeded),
            @"hostReconciled" : @(hostReconciled),
            @"secondPendingLoadAccepted" : @(secondPendingLoadAccepted),
            @"pendingHostCommitCalls" : @(pendingHostCommitCalls),
            @"pendingGenerationReconciled" : @(pendingGenerationReconciled),
            @"generationReadbackAPIAvailable" : @(generationReadbackAPIAvailable),
            @"staleHostReadbackApplied" : @(staleHostReadbackApplied),
            @"staleHostReadbackPreservedCurrentProject" :
                @(staleHostReadbackPreservedCurrentProject),
            @"freshHostReadbackReconciled" : @(freshHostReadbackReconciled),
            @"currentProjectGeneration" : @(store.currentProjectGeneration),
            @"renderRestoreSucceeded" : @(renderRestoreSucceeded),
            @"renderRestoreReplaced" : @(renderRestoreReplaced),
            @"readbackFailureClearedPending" : @(readbackFailureClearedPending),
            @"readbackFailurePreservedPrevious" : @(readbackFailurePreservedPrevious),
            @"retryAfterReadbackFailureSucceeded" : @(retryAfterReadbackFailureSucceeded),
            @"wrongGenerationTimeoutIgnored" : @(wrongGenerationTimeoutIgnored),
            @"matchingGenerationTimeoutApplied" : @(matchingGenerationTimeoutApplied),
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
