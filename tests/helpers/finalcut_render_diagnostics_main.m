#import <Foundation/Foundation.h>

#import "GFRenderDiagnostics.h"

int main(void) {
    @autoreleasepool {
        __block NSTimeInterval now = 0.0;
        NSMutableArray<NSString *> *logs = [NSMutableArray array];
        GFRenderDiagnostics *diagnostics = [[GFRenderDiagnostics alloc]
            initWithClock:^NSTimeInterval {
                return now;
            }
                   logger:^(NSString *message) {
                       [logs addObject:message];
                   }];
        for (NSUInteger frame = 0; frame < 1800; ++frame) {
            if (frame == 900) {
                now = 31.0;
            } else if (frame == 1799) {
                now = 62.0;
            }
            [diagnostics recordPassthroughStatus:4 reason:@"no project"];
        }
        [diagnostics recordPassthroughStatus:5 reason:@"invalid timing"];
        [diagnostics recordDeviceEnumeration];
        [diagnostics recordCommandQueueCreation];
        [diagnostics recordPipelineCreation];
        [diagnostics recordProjectDecode];
        [diagnostics recordProjectCacheHit];
        [diagnostics recordProjectCacheMiss];
        [diagnostics recordCacheInsertionBytes:4096];
        [diagnostics recordCacheInsertionBytes:2048];
        [diagnostics recordCacheEvictionBytes:2048];
        [diagnostics recordCachePurgeForMemoryPressure:YES];
        [diagnostics recordPluginStateBytes:100 encodeSeconds:0.001];
        [diagnostics recordPluginStateBytes:200 encodeSeconds:0.002];
        [diagnostics recordGPUSeconds:0.004];
        [diagnostics recordFrameSeconds:0.010 processed:YES];
        [diagnostics recordFrameSeconds:0.005 processed:NO];
        NSMutableDictionary *snapshot = [[diagnostics snapshot] mutableCopy];
        snapshot[@"captured_logs"] = @(logs.count);
        snapshot[@"first_log_has_reason"] = @([logs.firstObject containsString:@"no project"]);
        snapshot[@"last_log_has_new_reason"] = @([logs.lastObject containsString:@"invalid timing"]);
        NSData *json = [NSJSONSerialization dataWithJSONObject:snapshot
                                                       options:NSJSONWritingSortedKeys
                                                         error:NULL];
        fwrite(json.bytes, 1, json.length, stdout);
        fputc('\n', stdout);
        return 0;
    }
}
