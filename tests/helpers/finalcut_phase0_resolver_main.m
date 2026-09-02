#import <Foundation/Foundation.h>

#import "GFPhase0MediaResolver.h"


static int GFPrintResolution(NSDictionary<NSString *, id> *resolution, NSError *error) {
    if (resolution == nil) {
        fprintf(stderr, "%s\n", error.localizedDescription.UTF8String ?: "resolution failed");
        return 2;
    }
    NSData *json = [NSJSONSerialization dataWithJSONObject:resolution options:NSJSONWritingSortedKeys error:&error];
    if (json == nil) {
        fprintf(stderr, "%s\n", error.localizedDescription.UTF8String ?: "JSON encoding failed");
        return 3;
    }
    fwrite(json.bytes, 1, json.length, stdout);
    fputc('\n', stdout);
    return 0;
}


int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 3) {
            fprintf(stderr, "usage: resolver finder PATH... | resolver fcpxml XML_PATH\n");
            return 64;
        }

        NSString *mode = [NSString stringWithUTF8String:argv[1]];
        NSError *error = nil;
        NSDictionary<NSString *, id> *resolution = nil;
        if ([mode isEqualToString:@"finder"]) {
            NSMutableArray<NSURL *> *urls = [NSMutableArray array];
            for (int index = 2; index < argc; index++) {
                [urls addObject:[NSURL fileURLWithPath:[NSString stringWithUTF8String:argv[index]]]];
            }
            resolution = GFPhase0ResolveFinderURLs(urls, &error);
        } else if ([mode isEqualToString:@"fcpxml"] && argc == 3) {
            NSData *data = [NSData dataWithContentsOfFile:[NSString stringWithUTF8String:argv[2]]];
            resolution = GFPhase0ResolveFCPXMLData(data, &error);
        } else {
            fprintf(stderr, "invalid arguments\n");
            return 64;
        }
        return GFPrintResolution(resolution, error);
    }
}
