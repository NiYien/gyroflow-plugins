#import <FxPlug/FxPlugSDK.h>

@interface GyroflowFinalCutEffect
    : NSObject <FxTileableEffect, FxCustomParameterViewHost_v2>

@property(nonatomic, strong) id<PROAPIAccessing> apiManager;

- (NSView *)createViewForParameterID:(UInt32)parameterID NS_RETURNS_RETAINED;

@end
