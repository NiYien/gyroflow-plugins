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
        GFProjectStore *store = [[GFProjectStore alloc]
            initWithPayloadBuilder:^BOOL(
                NSData *projectData,
                NSString **payload,
                NSError **error
            ) {
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
                    return NO;
                }
                if (payload != NULL) {
                    *payload = [NSString stringWithFormat:@"payload-%lu",
                                                          (unsigned long)projectData.length];
                }
                return YES;
            }];
        NSMutableArray<NSString *> *errors = [NSMutableArray array];
        BOOL restoreSucceeded = NO;
        BOOL hostReconciled = NO;
        BOOL renderRestoreSucceeded = NO;
        BOOL renderRestoreReplaced = NO;
        if (argc == 4 && strcmp(argv[1], "--host-reject") == 0) {
            NSURL *previousURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[2]]];
            NSURL *replacementURL = [NSURL fileURLWithPath:
                [NSString stringWithUTF8String:argv[3]]];
            [store importProjectURL:previousURL error:NULL];
            NSString *previousPayload = store.currentProjectPayload;
            [store importProjectURL:replacementURL
                      commitPayload:^BOOL(NSString *projectPayload) {
                          (void)projectPayload;
                          return YES;
                      }
                              error:NULL];
            hostReconciled =
                [store reconcileHostPersistedProjectPayload:previousPayload ?: @""];
        }
        BOOL rejectNextCommit = NO;
        for (int index = argc == 4 && strcmp(argv[1], "--host-reject") == 0 ? argc : 1;
             index < argc;
             index++) {
            NSString *argument = [NSString stringWithUTF8String:argv[index]];
            if ([argument isEqualToString:@"--restore-no-timing"]) {
                GFAllowPayloadRestore = YES;
                NSError *error = nil;
                restoreSucceeded = [store
                    restorePersistedProjectPayload:@"persisted-payload"
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
                                                  timingPayload:@""];
                renderRestoreReplaced = [store
                    restoreValidatedRenderProjectPayloadIfEmpty:@"replacement-payload"
                                                  timingPayload:@"timing"];
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
                            commitPayload:^BOOL(NSString *projectPayload) {
                                (void)projectPayload;
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
            @"status" : store.status,
            @"errors" : errors,
            @"restoreSucceeded" : @(restoreSucceeded),
            @"hostReconciled" : @(hostReconciled),
            @"renderRestoreSucceeded" : @(renderRestoreSucceeded),
            @"renderRestoreReplaced" : @(renderRestoreReplaced),
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
