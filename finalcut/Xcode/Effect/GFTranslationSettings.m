// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFTranslationSettings.h"
#import <math.h>

BOOL GFTranslationParametersFromSettings(NSString *settings, NSString *projectIdentity,
                                         GFTranslationParameters *parameters) {
    *parameters = (GFTranslationParameters){0};
    if (settings.length == 0) { return YES; }
    if (settings.length > 4096) { return NO; }
    NSData *data = [settings dataUsingEncoding:NSUTF8StringEncoding];
    id value = data != nil ? [NSJSONSerialization JSONObjectWithData:data options:0 error:NULL] : nil;
    if (![value isKindOfClass:NSDictionary.class] || ![value[@"version"] isEqual:@1] ||
        ![value[@"project"] isKindOfClass:NSString.class]) { return NO; }
    if (![value[@"project"] isEqualToString:projectIdentity]) { return YES; }
    for (NSString *key in @[@"reference", @"smoothness", @"enabled", @"automatic", @"along_axis"]) {
        if (![value[key] isKindOfClass:NSNumber.class]) { return NO; }
    }
    double reference = [value[@"reference"] doubleValue], seconds = [value[@"smoothness"] doubleValue];
    if (!isfinite(reference) || reference < 0 || reference > 200 ||
        !isfinite(seconds) || seconds < 0.1 || seconds > 10) { return NO; }
    for (NSString *key in @[@"enabled", @"automatic", @"along_axis"]) {
        double flag = [value[key] doubleValue];
        if (flag != 0 && flag != 1) { return NO; }
    }
    *parameters = (GFTranslationParameters){
        .reference_percent = reference, .smoothness_seconds = seconds,
        .enabled = [value[@"enabled"] boolValue], .automatic = [value[@"automatic"] boolValue],
        .along_axis = [value[@"along_axis"] boolValue], .initialized = 1,
    };
    return YES;
}

NSString *GFTranslationSettingsString(GFTranslationParameters parameters, NSString *projectIdentity) {
    if (!isfinite(parameters.reference_percent) || parameters.reference_percent < 0 || parameters.reference_percent > 200 ||
        !isfinite(parameters.smoothness_seconds) || parameters.smoothness_seconds < 0.1 || parameters.smoothness_seconds > 10 ||
        parameters.enabled > 1 || parameters.automatic > 1 || parameters.along_axis > 1) { return nil; }
    NSDictionary *values = @{@"version": @1, @"project": projectIdentity,
        @"reference": @(parameters.reference_percent), @"smoothness": @(parameters.smoothness_seconds),
        @"enabled": @(parameters.enabled != 0), @"automatic": @(parameters.automatic != 0),
        @"along_axis": @(parameters.along_axis != 0)};
    NSData *data = [NSJSONSerialization dataWithJSONObject:values options:NSJSONWritingSortedKeys error:NULL];
    return data != nil ? [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding] : nil;
}
