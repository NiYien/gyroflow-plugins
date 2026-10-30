// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFUniformSourceTiming.h"
#import <AVFoundation/AVFoundation.h>
#import <zlib.h>
#import <math.h>

static const NSUInteger GFUniformMaxFrames = 1000000;
static const NSUInteger GFUniformMaxString = 416 * 1024;
static NSError *GFUniformError(NSString *text) {
    return [NSError errorWithDomain:@"com.niyien.finalcut.uniform" code:1
        userInfo:@{NSLocalizedDescriptionKey:text}];
}
static BOOL GFUniformInteger(id object, int64_t min, int64_t max) {
    if (![object isKindOfClass:NSNumber.class] || CFGetTypeID((__bridge CFTypeRef)object)==CFBooleanGetTypeID()) return NO;
    double value = [object doubleValue];
    return isfinite(value) && value >= min && value <= max && value == [object longLongValue];
}
static int64_t GFIdealTicks(NSUInteger n, int32_t scale) {
    return (int64_t)((__int128)n * 1001 * scale / 60000);
}

@interface GFUniformSourceTiming ()
@property(nonatomic) NSInteger samplingMode;
@property(nonatomic) NSUInteger frameCount;
@property(nonatomic) int32_t timeScale;
@property(nonatomic, copy) NSString *sourceName;
@property(nonatomic, copy) NSString *projectHash;
@property(nonatomic, copy) NSData *deltas;
@property(nonatomic, copy) NSDictionary *fields;
@end

@implementation GFUniformSourceTiming
+ (instancetype)timingFromString:(NSString *)string projectHash:(NSString *)hash {
    if (string.length == 0 || [string lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > GFUniformMaxString || hash.length != 64) return nil;
    static NSCache *cache; static dispatch_once_t once;
    dispatch_once(&once, ^{ cache = [NSCache new]; cache.totalCostLimit = 16 * 1024 * 1024; cache.countLimit = 64; });
    NSString *key = string;
    @synchronized(cache) {
        id existing = [cache objectForKey:key];
        if (existing) return [[(GFUniformSourceTiming *)existing projectHash] isEqualToString:hash] ? existing : nil;
        id root = [NSJSONSerialization JSONObjectWithData:[string dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
        GFUniformSourceTiming *value = nil;
        if ([root isKindOfClass:NSDictionary.class] && GFUniformInteger(root[@"v"],1,1) &&
            GFUniformInteger(root[@"mode"],1,2) && GFUniformInteger(root[@"count"],2,GFUniformMaxFrames) &&
            GFUniformInteger(root[@"scale"],60000,INT32_MAX) &&
            [root[@"hash"] isKindOfClass:NSString.class] && [root[@"hash"] isEqualToString:hash] &&
            [root[@"name"] isKindOfClass:NSString.class] && [root[@"name"] length] <= 1024 &&
            [root[@"delta"] isKindOfClass:NSString.class]) {
            NSUInteger count = [root[@"count"] unsignedIntegerValue];
            NSData *compressed = [[NSData alloc] initWithBase64EncodedString:root[@"delta"] options:0];
            NSMutableData *delta = [NSMutableData dataWithLength:count];
            z_stream stream = {0};
            stream.next_in = (Bytef *)compressed.bytes; stream.avail_in = (uInt)compressed.length;
            stream.next_out = delta.mutableBytes; stream.avail_out = (uInt)count;
            BOOL valid = compressed != nil && compressed.length > 0 && inflateInit(&stream) == Z_OK;
            if (valid) {
                valid = inflate(&stream,Z_FINISH) == Z_STREAM_END && stream.total_out == count && stream.avail_in == 0;
                inflateEnd(&stream);
            }
            int32_t scale = [root[@"scale"] intValue];
            const int8_t *d = delta.bytes; int64_t previous = -1;
            for (NSUInteger n = 0; valid && n < count; n++) {
                int64_t actual = GFIdealTicks(n,scale) + d[n];
                __int128 deviation = (__int128)actual * 60000 - (__int128)n * 1001 * scale;
                valid = d[n] >= -1 && d[n] <= 1 && actual > previous && (n != 0 || actual == 0) &&
                    deviation >= -60000 && deviation <= 60000;
                previous = actual;
            }
            if (valid) {
                value = [GFUniformSourceTiming new]; value.samplingMode = [root[@"mode"] integerValue];
                value.frameCount = count; value.timeScale = scale; value.deltas = delta;
                value.projectHash = hash; value.sourceName = root[@"name"]; value.fields = root;
            }
        }
        if (value) [cache setObject:value forKey:key cost:value.frameCount + string.length * 2];
        return value;
    }
}
+ (NSString *)prepareURL:(NSURL *)url projectData:(NSData *)projectData
            projectHash:(NSString *)hash mode:(NSInteger)mode error:(NSError **)error {
    id root = [NSJSONSerialization JSONObjectWithData:projectData options:0 error:nil];
    id video = [root isKindOfClass:NSDictionary.class] ? root[@"video_info"] : nil;
    if (![video isKindOfClass:NSDictionary.class] || ![video[@"fps"] isKindOfClass:NSNumber.class] ||
        fabs([video[@"fps"] doubleValue] - 60000.0/1001.0) > 0.000001 ||
        !GFUniformInteger(video[@"width"],1,65536) || !GFUniformInteger(video[@"height"],1,65536) ||
        !GFUniformInteger(video[@"rotation"] ?: @0,-360,360) || !GFUniformInteger(video[@"num_frames"],2,GFUniformMaxFrames) || hash.length != 64 || mode < 1 || mode > 2) {
        if (error) *error = GFUniformError(@"This profile requires a matching 59.94p constant-frame-rate .gyroflow project.");
        return nil;
    }
    AVURLAsset *asset = [AVURLAsset URLAssetWithURL:url options:@{AVURLAssetPreferPreciseDurationAndTimingKey:@YES}];
    NSArray<AVAssetTrack *> *tracks = [asset tracksWithMediaType:AVMediaTypeVideo];
    if (tracks.count != 1 || !tracks.firstObject.canProvideSampleCursors) {
        if (error) *error = GFUniformError(@"The selected original video does not provide a single sample timing track.");
        return nil;
    }
    AVAssetTrack *track = tracks.firstObject;
    CGSize size = track.naturalSize;
    double w = [video[@"width"] doubleValue], h = [video[@"height"] doubleValue];
    BOOL dimensions = fabs(size.width-w)<0.5 && fabs(size.height-h)<0.5;
    double rotation = [video[@"rotation"] doubleValue];
    if (fabs(remainder(rotation,180.0)) == 90.0)
        dimensions |= fabs(size.width-h)<0.5 && fabs(size.height-w)<0.5;
    if (!dimensions || !isfinite(track.nominalFrameRate) || fabs(track.nominalFrameRate-60000.0/1001.0)>0.001) {
        if (error) *error = GFUniformError(@"The original video dimensions or frame rate do not match this project.");
        return nil;
    }
    int32_t scale = track.naturalTimeScale;
    AVSampleCursor *cursor = [track makeSampleCursorAtFirstSampleInDecodeOrder];
    [cursor stepInPresentationOrderByCount:-INT64_MAX];
    CMTime origin = cursor.presentationTimeStamp;
    if (!cursor || !CMTIME_IS_NUMERIC(origin) || scale < 60000) {
        if (error) *error = GFUniformError(@"The original video sample time base is unsupported."); return nil;
    }
    NSMutableData *delta = [NSMutableData data]; BOOL valid = YES;
    do {
        NSUInteger n = delta.length;
        CMTime relative = CMTimeSubtract(cursor.presentationTimeStamp,origin);
        CMTime ticks = CMTimeConvertScale(relative,scale,kCMTimeRoundingMethod_Default);
        __int128 deviation = (__int128)ticks.value * 60000 - (__int128)n * 1001 * scale;
        int64_t difference = ticks.value - GFIdealTicks(n,scale);
        if (n >= GFUniformMaxFrames || !CMTIME_IS_NUMERIC(relative) || CMTimeCompare(relative,ticks) != 0 ||
            deviation < -60000 || deviation > 60000 || difference < -1 || difference > 1) { valid = NO; break; }
        int8_t d = (int8_t)difference; [delta appendBytes:&d length:1];
    } while ([cursor stepInPresentationOrderByCount:1] == 1);
    if (!valid || delta.length != [video[@"num_frames"] unsignedIntegerValue]) {
        if (error) *error = GFUniformError(@"The original video's sample count/timing is not the matching constant-frame-rate source."); return nil;
    }
    uLongf length = compressBound((uLong)delta.length);
    NSMutableData *compressed = [NSMutableData dataWithLength:length];
    if (compress2(compressed.mutableBytes,&length,delta.bytes,(uLong)delta.length,Z_BEST_COMPRESSION) != Z_OK) return nil;
    compressed.length = length;
    NSDictionary *fields = @{@"v":@1,@"mode":@(mode),@"hash":hash,@"name":url.lastPathComponent,
        @"count":@(delta.length),@"scale":@(scale),@"delta":[compressed base64EncodedStringWithOptions:0]};
    NSString *string = [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:fields options:NSJSONWritingSortedKeys error:error] encoding:NSUTF8StringEncoding];
    if (![self timingFromString:string projectHash:hash]) {
        if (error) *error = GFUniformError(@"The source timing table cannot be saved within the plugin parameter limit."); return nil;
    }
    return string;
}
- (NSString *)stringWithMode:(NSInteger)mode {
    if (mode == 0) return @"";
    if (mode < 1 || mode > 2) return @"";
    NSMutableDictionary *fields = [self.fields mutableCopy]; fields[@"mode"] = @(mode);
    return [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:fields options:NSJSONWritingSortedKeys error:nil] encoding:NSUTF8StringEncoding];
}
- (NSInteger)domainForHostFrame:(CMTime)frame timelineFrame:(CMTime)timeline inputDuration:(CMTime)duration scale:(GFTime)scale {
    if (!CMTIME_IS_NUMERIC(frame) || !CMTIME_IS_NUMERIC(timeline) || !CMTIME_IS_NUMERIC(duration) ||
        scale.numerator != scale.denominator || scale.numerator <= 0 ||
        fabs(CMTimeGetSeconds(duration) - self.frameCount * 1001.0/60000.0) > 0.0001) return 0;
    CMTime native = CMTimeMake(1001,60000), conformed = CMTimeMake(1001,30000);
    if (CMTimeCompare(frame,native)==0 && CMTimeCompare(timeline,native)==0) return 1;
    if (CMTimeCompare(frame,conformed)==0 && CMTimeCompare(timeline,conformed)==0) return 2;
    return 0;
}
- (CMTime)sourceTime:(CMTime)time domain:(NSInteger)domain {
    if ((domain != 1 && domain != 2) || !CMTIME_IS_NUMERIC(time) || time.timescale <= 0 || time.epoch != 0) return time;
    CMTime lookup = time;
    // Only the conformed input applies Nearest's half-output-frame sampling shift.
    // A native compound already receives its outer sampler's shifted clock.
    if (domain == 2 && self.samplingMode == 2) lookup = CMTimeAdd(time,CMTimeMake(1001,60000));
    if (!CMTIME_IS_NUMERIC(lookup) || CMTimeCompare(time,kCMTimeZero) < 0 ||
        CMTimeCompare(time,CMTimeMake((int64_t)self.frameCount*1001,60000)) >= 0) return time;
    const int8_t *d = self.deltas.bytes;
    NSUInteger low = 0, high = self.frameCount;
    while (low < high) {
        NSUInteger mid = low + (high-low)/2;
        CMTime pts = CMTimeMake(GFIdealTicks(mid,self.timeScale)+d[mid],self.timeScale);
        if (CMTimeCompare(pts,lookup) <= 0) low = mid+1; else high = mid;
    }
    return low > 0 ? CMTimeMake((int64_t)(low-1)*1001,60000) : time;
}
@end
