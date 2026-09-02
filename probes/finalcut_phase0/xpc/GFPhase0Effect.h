#import <Foundation/Foundation.h>
#import <FxPlug/FxPlugSDK.h>

@class GFPhase0ProjectStore;

double GFPhase0LastRenderTimestampSeconds(void);


@interface GFPhase0Effect : NSObject <FxTileableEffect, FxCustomParameterViewHost_v2>

@property(nonatomic, strong) id<PROAPIAccessing> apiManager;
@property(nonatomic, strong) GFPhase0ProjectStore *projectStore;

- (NSView *)createViewForParameterID:(UInt32)parameterID NS_RETURNS_RETAINED;

@end
