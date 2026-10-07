// SPDX-License-Identifier: GPL-3.0-or-later
#import <Foundation/Foundation.h>
#import "GyroflowFinalCut.h"

BOOL GFTranslationParametersFromSettings(NSString *settings, NSString *projectIdentity,
                                         GFTranslationParameters *parameters);
NSString *GFTranslationSettingsString(GFTranslationParameters parameters, NSString *projectIdentity);
