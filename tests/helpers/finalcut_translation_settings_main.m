// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import "GFTranslationSettings.h"
#include <math.h>
#include <string.h>

int main(void) {
    @autoreleasepool {
        GFTranslationParameters original = {
            .reference_percent = 70, .smoothness_seconds = 2.5,
            .enabled = 0, .automatic = 1, .along_axis = 1, .initialized = 1,
        };
        GFTranslationParameters decoded = {0};
        NSString *settings = GFTranslationSettingsString(original, @"project-1");
        NSCAssert(GFTranslationParametersFromSettings(settings, @"project-1", &decoded), @"saved state decodes");
        NSCAssert(memcmp(&original, &decoded, sizeof(original)) == 0, @"disabled and manual values survive saving");
        NSCAssert(GFTranslationParametersFromSettings(settings, @"project-2", &decoded), @"another project ignores old overrides");
        NSCAssert(decoded.initialized == 0, @"explicit reload uses project defaults");
        NSCAssert(GFTranslationParametersFromSettings(@"", @"project-1", &decoded), @"legacy state is accepted");
        NSCAssert(decoded.initialized == 0, @"legacy state preserves imported values");
        NSCAssert(!GFTranslationParametersFromSettings(@"{}", @"project-1", &decoded), @"invalid schema is rejected");
        NSCAssert(!GFTranslationParametersFromSettings(@"[]", @"project-1", &decoded), @"invalid type is rejected");
        NSMutableDictionary *invalid = [[NSJSONSerialization JSONObjectWithData:
            [settings dataUsingEncoding:NSUTF8StringEncoding] options:NSJSONReadingMutableContainers error:NULL] mutableCopy];
        for (NSString *key in @[@"reference", @"smoothness", @"enabled"]) {
            NSMutableDictionary *values = [invalid mutableCopy];
            values[key] = @300;
            NSData *data = [NSJSONSerialization dataWithJSONObject:values options:0 error:NULL];
            NSString *text = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
            NSCAssert(!GFTranslationParametersFromSettings(text, @"project-1", &decoded), @"invalid values are rejected");
        }
        original.smoothness_seconds = NAN;
        NSCAssert(GFTranslationSettingsString(original, @"project-1") == nil, @"non-finite values cannot be saved");
    }
    return 0;
}
