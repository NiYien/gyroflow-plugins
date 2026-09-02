#import "RouteDProcessor.h"

#import "GyroflowFinalCut.h"

static NSString *const GFRouteDProcessorErrorDomain =
    @"com.niyien.gyroflow.finalcut.route-d";

@implementation RouteDProcessor

+ (nullable NSDictionary<NSString *, NSData *> *)resultForStatus:(GFStatus)status
                                                           result:(GFRouteDPatchResult *)result
                                                      bridgeError:(GFError *)bridgeError
                                                            error:(NSError **)error {
    if (status != GF_STATUS_OK) {
        NSString *message = bridgeError != NULL && bridgeError->message != NULL
            ? [NSString stringWithUTF8String:bridgeError->message]
            : @"Unable to process Final Cut project XML";
        if (error != NULL) {
            *error = [NSError errorWithDomain:GFRouteDProcessorErrorDomain
                                         code:status
                                     userInfo:@{
                                         NSLocalizedDescriptionKey :
                                             message ?: @"Unknown Route D error"
                                     }];
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

+ (nullable NSDictionary<NSString *, NSData *> *)processFCPXML:(NSData *)input
                                                 processedName:(NSString *)processedName
                                                         error:(NSError **)error {
    NSData *name = [processedName dataUsingEncoding:NSUTF8StringEncoding];
    GFRouteDPatchResult result = {0};
    GFError *bridgeError = NULL;
    GFStatus status = gf_finalcut_route_d_patch(
        input.bytes,
        input.length,
        name.bytes,
        name.length,
        &result,
        &bridgeError
    );
    return [self resultForStatus:status
                         result:&result
                    bridgeError:bridgeError
                          error:error];
}

+ (nullable NSDictionary<NSString *, NSData *> *)processBatchFCPXML:(NSData *)input
                                                      processedName:(NSString *)processedName
                                                              error:(NSError **)error {
    NSData *name = [processedName dataUsingEncoding:NSUTF8StringEncoding];
    GFRouteDPatchResult result = {0};
    GFError *bridgeError = NULL;
    GFStatus status = gf_finalcut_route_d_batch_patch(
        input.bytes,
        input.length,
        name.bytes,
        name.length,
        &result,
        &bridgeError
    );
    return [self resultForStatus:status
                         result:&result
                    bridgeError:bridgeError
                          error:error];
}

@end
