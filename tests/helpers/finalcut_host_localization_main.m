// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import "GFLocalization.h"

int main(int argc, const char *argv[]) {
    @autoreleasepool {
        if (argc < 2) { return 64; }
        NSData *input = [[NSString stringWithUTF8String:argv[1]] dataUsingEncoding:NSUTF8StringEncoding];
        NSArray *cases = [NSJSONSerialization JSONObjectWithData:input options:0 error:nil];
        id before = [NSUserDefaults.standardUserDefaults objectForKey:@"AppleLanguages"];
        NSMutableArray *results = [NSMutableArray array];
        for (NSDictionary *item in cases) {
            NSArray *languages = GFHostLanguagePreferences(item[@"host"], item[@"home"], item[@"system"]);
            GFSetLocalizationLanguages(languages);
            [results addObject:@{@"languages": languages, @"text": GFLocalized(@"probe.key", @"fallback")}];
        }
        id after = [NSUserDefaults.standardUserDefaults objectForKey:@"AppleLanguages"];
        NSDictionary *report = @{@"results": results, @"before": before ?: [NSNull null], @"after": after ?: [NSNull null]};
        NSData *output = [NSJSONSerialization dataWithJSONObject:report options:0 error:nil];
        puts([[NSString alloc] initWithData:output encoding:NSUTF8StringEncoding].UTF8String);
    }
    return 0;
}
