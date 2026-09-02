#import <Foundation/Foundation.h>

#import "GFPhase0ProjectStore.h"


int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 2) {
            fprintf(stderr, "usage: project-store PATH|--cancel ...\n");
            return 64;
        }

        GFPhase0ProjectStore *store = [[GFPhase0ProjectStore alloc] init];
        NSMutableArray<NSString *> *errors = [NSMutableArray array];
        for (int index = 1; index < argc; index++) {
            NSString *argument = [NSString stringWithUTF8String:argv[index]];
            if ([argument isEqualToString:@"--cancel"]) {
                [store recordAuthorizationCancellation];
                continue;
            }
            NSError *error = nil;
            NSURL *url = [NSURL fileURLWithPath:argument];
            if (![store importProjectURL:url error:&error]) {
                [errors addObject:error.localizedDescription ?: @"unknown import error"];
            }
        }

        NSDictionary<NSString *, id> *result = @{
            @"currentBytes" : @(store.currentProjectData.length),
            @"currentVersion" : store.currentProjectVersion ?: [NSNull null],
            @"status" : store.status,
            @"errors" : errors,
        };
        NSError *jsonError = nil;
        NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                       options:NSJSONWritingSortedKeys
                                                         error:&jsonError];
        if (json == nil) {
            fprintf(stderr, "%s\n", jsonError.localizedDescription.UTF8String ?: "JSON encoding failed");
            return 3;
        }
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
