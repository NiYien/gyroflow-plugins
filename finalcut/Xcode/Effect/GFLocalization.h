// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

FOUNDATION_EXPORT NSString *GFLocalized(NSString *key, NSString *fallback);
FOUNDATION_EXPORT void GFSetLocalizationLanguages(NSArray<NSString *> *languages);
FOUNDATION_EXPORT void GFConfigureLocalizationForHost(NSString *identifier);
FOUNDATION_EXPORT NSArray<NSString *> *GFHostLanguagePreferences(
    NSString *identifier,
    NSString *homeDirectory,
    NSArray<NSString *> *systemLanguages
);

NS_ASSUME_NONNULL_END
