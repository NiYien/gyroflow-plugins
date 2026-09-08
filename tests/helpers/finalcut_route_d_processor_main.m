#import <Foundation/Foundation.h>
#import <stdlib.h>
#import <string.h>

#import "RouteDProcessor.h"
#import "GyroflowFinalCut.h"

static GFStatus GFNextStatus = GF_STATUS_ROUTE_D_INVALID_INPUT;
static NSArray<NSDictionary *> *GFCapturedInputs = @[];

GFStatus gf_finalcut_route_d_batch_patch_with_project_inputs(
    const uint8_t *input_bytes,
    size_t input_len,
    const GFRouteDProjectInput *project_inputs,
    size_t project_inputs_len,
    GFRouteDPatchResult *out_result,
    GFError **out_error
) {
    (void)input_bytes;
    (void)input_len;
    *out_result = (GFRouteDPatchResult){0};
    NSMutableArray<NSDictionary *> *captured = [NSMutableArray array];
    for (size_t index = 0; index < project_inputs_len; index++) {
        const GFRouteDProjectInput *project = &project_inputs[index];
        NSString *path = [[NSString alloc]
            initWithBytes:project->path_bytes
                   length:project->path_len
                 encoding:NSUTF8StringEncoding] ?: @"";
        [captured addObject:@{
            @"path" : path,
            @"status" : @(project->status),
            @"length" : @(project->project_len),
        }];
    }
    GFCapturedInputs = captured;
    GFError *error = calloc(1, sizeof(GFError));
    error->code = GFNextStatus;
    error->message = strdup("raw Rust diagnostic must not be user-visible");
    *out_error = error;
    return GFNextStatus;
}

void gf_finalcut_error_free(GFError *error) {
    if (error != NULL) {
        free(error->message);
        free(error);
    }
}

void gf_finalcut_route_d_patch_result_free(GFRouteDPatchResult *result) {
    (void)result;
}

int main(void) {
    @autoreleasepool {
        NSMutableArray *results = [NSMutableArray array];
        NSURL *documentURL = [NSURL fileURLWithPath:@"/tmp/Input.fcpxml"];
        NSURL *fixtureRoot = [[NSURL fileURLWithPath:NSTemporaryDirectory()
                                       isDirectory:YES]
            URLByAppendingPathComponent:NSUUID.UUID.UUIDString
                             isDirectory:YES];
        [NSFileManager.defaultManager createDirectoryAtURL:fixtureRoot
                               withIntermediateDirectories:YES
                                                attributes:nil
                                                     error:NULL];
        NSURL *projectURL = [fixtureRoot URLByAppendingPathComponent:@"A.gyroflow"];
        NSData *projectData = [@"direct project" dataUsingEncoding:NSUTF8StringEncoding];
        [projectData writeToURL:projectURL atomically:YES];
        NSURL *mediaURL = [[projectURL URLByDeletingPathExtension]
            URLByAppendingPathExtension:@"mov"];
        NSData *directInput = [[NSString stringWithFormat:
            @"<fcpxml><resources><asset><media-rep kind='original-media' src='%@'/></asset></resources></fcpxml>",
            mediaURL.absoluteString]
            dataUsingEncoding:NSUTF8StringEncoding];
        GFNextStatus = GF_STATUS_ROUTE_D_INVALID_INPUT;
        [RouteDProcessor processBatchFCPXML:directInput
                                documentURL:documentURL
                                      error:NULL];
        NSDictionary *capturedProject = GFCapturedInputs.firstObject;
        BOOL directProjectRead =
            [capturedProject[@"path"] isEqualToString:projectURL.path] &&
            [capturedProject[@"status"] unsignedIntValue] ==
                GF_ROUTE_D_PROJECT_INPUT_AVAILABLE &&
            [capturedProject[@"length"] unsignedIntegerValue] == projectData.length;
        [NSFileManager.defaultManager removeItemAtURL:fixtureRoot error:NULL];

        NSData *input = [@"<fcpxml><resources><asset><media-rep kind='original-media' src='file:///Media/A.mov'><bookmark>invalid</bookmark></media-rep></asset></resources></fcpxml>"
            dataUsingEncoding:NSUTF8StringEncoding];
        NSArray<NSNumber *> *statuses = @[
            @(GF_STATUS_ROUTE_D_INVALID_INPUT),
            @(GF_STATUS_ROUTE_D_UNSAFE_STRUCTURE),
            @(GF_STATUS_ROUTE_D_NO_UPDATEABLE_TARGETS),
            @(GF_STATUS_INVALID_ARGUMENT),
        ];
        for (NSNumber *status in statuses) {
            GFNextStatus = (GFStatus)status.intValue;
            NSError *error = nil;
            [RouteDProcessor processBatchFCPXML:input
                                    documentURL:documentURL
                                          error:&error];
            [results addObject:@{
                @"code" : @(error.code),
                @"description" : error.localizedDescription ?: @"",
                @"diagnostic" : error.userInfo[NSDebugDescriptionErrorKey] ?: @"",
            }];
        }
        NSDictionary *output = @{
            @"results" : results,
            @"inputs" : GFCapturedInputs,
            @"directProjectRead" : @(directProjectRead),
        };
        NSData *json = [NSJSONSerialization dataWithJSONObject:output
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
