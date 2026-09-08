// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFLocalization.h"
#import <fcntl.h>
#import <os/log.h>
#import <pwd.h>
#import <sys/stat.h>
#import <unistd.h>

@interface GFLocalizationBundleMarker : NSObject
@end

@implementation GFLocalizationBundleMarker
@end

static NSBundle *GFHostLocalizationBundle;

static NSArray<NSString *> *GFValidLanguages(id value) {
    if (![value isKindOfClass:[NSArray class]] || [value count] == 0) {
        return nil;
    }
    for (id language in value) {
        if (![language isKindOfClass:[NSString class]] || [language length] == 0) {
            return nil;
        }
    }
    return [value copy];
}

static BOOL GFSupportedLanguageHost(NSString *identifier) {
    return [identifier isEqualToString:@"com.apple.FinalCut"]
        || [identifier isEqualToString:@"com.apple.FinalCutApp"];
}

static NSDictionary *GFReadHostPreferences(NSString *path, BOOL *exists) {
    int descriptor = open(path.fileSystemRepresentation, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (descriptor < 0) {
        *exists = errno != ENOENT;
        return nil;
    }
    *exists = YES;
    struct stat metadata;
    if (fstat(descriptor, &metadata) != 0 || !S_ISREG(metadata.st_mode)
        || metadata.st_uid != getuid() || metadata.st_size <= 0
        || metadata.st_size > 4 * 1024 * 1024) {
        close(descriptor);
        return nil;
    }
    NSFileHandle *file = [[NSFileHandle alloc] initWithFileDescriptor:descriptor
                                                     closeOnDealloc:YES];
    NSData *data = [file readDataUpToLength:(NSUInteger)metadata.st_size error:nil];
    [file closeFile];
    if (data.length != (NSUInteger)metadata.st_size) {
        return nil;
    }
    id value = [NSPropertyListSerialization propertyListWithData:data
                                                       options:NSPropertyListImmutable
                                                        format:nil
                                                         error:nil];
    return [value isKindOfClass:[NSDictionary class]] ? value : nil;
}

NSArray<NSString *> *GFHostLanguagePreferences(
    NSString *identifier,
    NSString *homeDirectory,
    NSArray<NSString *> *systemLanguages
) {
    NSArray<NSString *> *fallback = GFValidLanguages(systemLanguages) ?: @[@"en"];
    if (!GFSupportedLanguageHost(identifier) || homeDirectory.length == 0) {
        return fallback;
    }
    NSString *relative = [NSString stringWithFormat:
        @"Library/Containers/%@/Data/Library/Preferences/%@.plist", identifier, identifier];
    BOOL containerExists = NO;
    NSDictionary *preferences = GFReadHostPreferences(
        [homeDirectory stringByAppendingPathComponent:relative], &containerExists);
    if (containerExists) {
        // A removed override must use system preferences, not an older domain.
        return GFValidLanguages(preferences[@"AppleLanguages"]) ?: fallback;
    }
    id legacyLanguages = CFBridgingRelease(CFPreferencesCopyValue(
        CFSTR("AppleLanguages"), (__bridge CFStringRef)identifier,
        kCFPreferencesCurrentUser, kCFPreferencesAnyHost));
    return GFValidLanguages(legacyLanguages) ?: fallback;
}

void GFSetLocalizationLanguages(NSArray<NSString *> *languages) {
    NSBundle *bundle = [NSBundle bundleForClass:[GFLocalizationBundleMarker class]];
    NSArray<NSString *> *preferences = [(GFValidLanguages(languages) ?: @[])
        arrayByAddingObject:@"en"];
    NSString *region = [NSBundle preferredLocalizationsFromArray:bundle.localizations
                                                forPreferences:preferences].firstObject;
    NSString *path = [bundle pathForResource:region ?: @"en" ofType:@"lproj"];
    NSBundle *localizedBundle = path ? [NSBundle bundleWithPath:path] : nil;
    @synchronized ([GFLocalizationBundleMarker class]) {
        GFHostLocalizationBundle = localizedBundle;
    }
}

void GFConfigureLocalizationForHost(NSString *identifier) {
    struct passwd entry, *result = NULL;
    char buffer[16384];
    int status = getpwuid_r(getuid(), &entry, buffer, sizeof(buffer), &result);
    NSString *home = status == 0 && result != NULL
        ? [NSString stringWithUTF8String:entry.pw_dir] : @"";
    id global = CFBridgingRelease(CFPreferencesCopyValue(
        CFSTR("AppleLanguages"), kCFPreferencesAnyApplication,
        kCFPreferencesCurrentUser, kCFPreferencesAnyHost));
    NSArray<NSString *> *languages = GFHostLanguagePreferences(
        identifier, home, GFValidLanguages(global) ?: NSLocale.preferredLanguages);
    GFSetLocalizationLanguages(languages);
    os_log_info(OS_LOG_DEFAULT, "NiYien FCP host language host=%{public}@ preferences=%{public}@",
                identifier, languages);
}

NSString *GFLocalized(NSString *key, NSString *fallback) {
    NSBundle *bundle;
    @synchronized ([GFLocalizationBundleMarker class]) {
        bundle = GFHostLocalizationBundle;
    }
    bundle = bundle ?: [NSBundle bundleForClass:[GFLocalizationBundleMarker class]];
    NSString *localized = [bundle localizedStringForKey:key value:fallback table:nil];
    return localized.length > 0 ? localized : fallback;
}
