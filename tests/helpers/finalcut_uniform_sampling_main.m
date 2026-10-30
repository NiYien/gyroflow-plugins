// SPDX-License-Identifier: GPL-3.0-or-later
#import "GFUniformSourceTiming.h"
#import "GFRenderState.h"
#import <zlib.h>
static NSUInteger checks=0;
#define CHECK(x) do { checks++; if (!(x)) { fprintf(stderr,"Failed line %d: %s\n",__LINE__,#x); return 1; } } while(0)
static NSString *JSON(id value) { return [[NSString alloc] initWithData:[NSJSONSerialization dataWithJSONObject:value options:0 error:nil] encoding:NSUTF8StringEncoding]; }
int main(void) { @autoreleasepool {
    NSString *hash=[@"a" stringByPaddingToLength:64 withString:@"a" startingAtIndex:0];
    for (NSUInteger hv=0;hv<2;hv++) {
        uint8_t deltas[240];
        for(int n=0;n<240;n++) deltas[n]=hv?(n%2):0;
        uLongf length=compressBound(240); NSMutableData *compressed=[NSMutableData dataWithLength:length];
        CHECK(compress2(compressed.mutableBytes,&length,deltas,240,Z_BEST_COMPRESSION)==Z_OK);
        compressed.length=length;
        NSString *s=JSON(@{@"v":@1,@"mode":@2,@"count":@240,@"scale":hv?@90000:@60000,
            @"hash":hash,@"name":@"original.mov",@"delta":[compressed base64EncodedStringWithOptions:0]});
        GFUniformSourceTiming *near=[GFUniformSourceTiming timingFromString:s projectHash:hash];
        GFUniformSourceTiming *floor=[GFUniformSourceTiming timingFromString:[near stringWithMode:1] projectHash:hash];
        CHECK(near.frameCount==240 && floor.samplingMode==1);
        for (int k=0;k<120;k++) {
            CMTime output=CMTimeMake(k*1001,30000), nativeOuter=CMTimeMake((4*k+1)*1001,120000);
            CHECK(CMTimeCompare([near sourceTime:output domain:2],CMTimeMake((2*k+(hv?0:1))*1001,60000))==0);
            CHECK(CMTimeCompare([floor sourceTime:output domain:2],CMTimeMake(2*k*1001,60000))==0);
            CHECK(CMTimeCompare([near sourceTime:nativeOuter domain:1],CMTimeMake(2*k*1001,60000))==0);
            CHECK(CMTimeCompare([near sourceTime:nativeOuter domain:0],nativeOuter)==0);
        }
        for (int k=0;k<119;k++) {
            // FCP's odd-source outer-trim clock has a 1/60000 second positive tick.
            CMTime trim=CMTimeMake((2*k+1)*1001+1,60000);
            CHECK(CMTimeCompare([floor sourceTime:trim domain:1],CMTimeMake((2*k+1)*1001,60000))==0);
        }
        GFTime one={1,1}, two={2,1}; CMTime native=CMTimeMake(1001,60000), lower=CMTimeMake(1001,30000), duration=CMTimeMake(240*1001,60000);
        CHECK([near domainForHostFrame:native timelineFrame:native inputDuration:duration scale:one]==1);
        CHECK([near domainForHostFrame:lower timelineFrame:lower inputDuration:duration scale:one]==2);
        CHECK([near domainForHostFrame:CMTimeMake(1001,24000) timelineFrame:CMTimeMake(1001,24000) inputDuration:duration scale:one]==0);
        CHECK([near domainForHostFrame:lower timelineFrame:lower inputDuration:CMTimeMultiplyByRatio(duration,1,2) scale:one]==0);
        CHECK([near domainForHostFrame:lower timelineFrame:lower inputDuration:duration scale:two]==0);
        for (int domain=0;domain<3;domain++) {
            CMTime before=CMTimeMake(-1001,120000);
            CHECK(CMTimeCompare([near sourceTime:before domain:domain],before)==0);
            CHECK(CMTimeCompare([near sourceTime:duration domain:domain],duration)==0);
            CHECK(CMTIME_IS_INVALID([near sourceTime:kCMTimeInvalid domain:domain]));
        }
        NSString *other=[@"b" stringByPaddingToLength:64 withString:@"b" startingAtIndex:0];
        CHECK([GFUniformSourceTiming timingFromString:s projectHash:other]==nil);
        CHECK([GFUniformSourceTiming timingFromString:s projectHash:hash]==near);
        NSMutableDictionary *fields=[[NSJSONSerialization JSONObjectWithData:[s dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil] mutableCopy];
        for (NSString *key in @[@"count",@"scale",@"v",@"mode",@"delta",@"name",@"hash"]) {
            NSMutableDictionary *bad=[fields mutableCopy];bad[key]=@[];
            CHECK([GFUniformSourceTiming timingFromString:JSON(bad) projectHash:hash]==nil);
        }
        for (NSNumber *count in @[@0,@1,@1000001,@YES,@2.5]) {
            NSMutableDictionary *bad=[fields mutableCopy];bad[@"count"]=count;
            CHECK([GFUniformSourceTiming timingFromString:JSON(bad) projectHash:hash]==nil);
        }
        NSMutableDictionary *bad=[fields mutableCopy];bad[@"delta"]=[fields[@"delta"] stringByAppendingString:@"AAAA"];
        CHECK([GFUniformSourceTiming timingFromString:JSON(bad) projectHash:hash]==nil);
        CHECK([near stringWithMode:0].length==0);
        GFRenderState *state=[[GFRenderState alloc] initWithProjectPayload:@"payload" timingPayload:@""
            parameters:(GFRenderParameters){.fov=1.37} effectBounds:(GFTimeRange){0} inputBounds:(GFTimeRange){0}];
        state=[[GFRenderState alloc] initWithState:state uniformSourceTiming:s domain:2];
        state=[[GFRenderState alloc] initWithState:state hostOptions:(GFHostOptions){0} sourceTimeScale:one];
        state=[[GFRenderState alloc] initWithState:state translationSettings:@"{}" projectIdentity:@"keep"];
        NSData *archive=[NSKeyedArchiver archivedDataWithRootObject:state requiringSecureCoding:YES error:nil];
        GFRenderState *restored=[NSKeyedUnarchiver unarchivedObjectOfClass:GFRenderState.class fromData:archive error:nil];
        CHECK(restored.uniformDomain==2 && [restored.uniformSourceTiming isEqualToString:s] && restored.parameters.fov==1.37);
    }
    NSData *legacy=[[NSData alloc] initWithBase64EncodedString:@"YnBsaXN0MDDUAQIDBAUGBwpYJHZlcnNpb25ZJGFyY2hpdmVyVCR0b3BYJG9iamVjdHMSAAGGoF8QD05TS2V5ZWRBcmNoaXZlctEICVRyb290gAGmCww3ODk6VSRudWxs3xAcDQ4PEBESExQVFhcYGRobHB0eHyAhIiMkJSYnKCkqKywrKi0tKisuKystLSwwMSssMy40NSsrLixYb3ZlcnZpZXdYem9vbU1vZGVfEBllZmZlY3REdXJhdGlvbkRlbm9taW5hdG9yXxAadHJhbnNsYXRpb25Qcm9qZWN0SWRlbnRpdHlfEBNpbnB1dFN0YXJ0TnVtZXJhdG9yXxAQaW5wdXRPcmllbnRhdGlvbl5sZW5zQ29ycmVjdGlvblpzbW9vdGhuZXNzWmhvc3RTaXppbmdfEBVpbnB1dFN0YXJ0RGVub21pbmF0b3JfEBpzb3VyY2VUaW1lU2NhbGVEZW5vbWluYXRvcl8QFmVmZmVjdFN0YXJ0RGVub21pbmF0b3JfEBhpbnB1dER1cmF0aW9uRGVub21pbmF0b3JfEA9ob3Jpem9uTG9ja1JvbGxfEBFob3Jpem9uTG9ja0Ftb3VudF10aW1pbmdQYXlsb2FkXnByb2plY3RQYXlsb2FkViRjbGFzc18QFGVmZmVjdFN0YXJ0TnVtZXJhdG9yXxAScHJvamVjdERpc3BsYXlOYW1lVnNjaGVtYV8QGHNvdXJjZVRpbWVTY2FsZU51bWVyYXRvclNmb3ZfEBJwcm9qZWN0Q29udGVudEhhc2hfEBZpbnB1dER1cmF0aW9uTnVtZXJhdG9yXxAXZWZmZWN0RHVyYXRpb25OdW1lcmF0b3JUbW9kZV8QE3RyYW5zbGF0aW9uU2V0dGluZ3MIEAAQAIACIwAAAAAAAAAAEAGAAoADgAWAAhAFIz/7MzMzMzMzgASAAlBXcGF5bG9hZF8QQDIzOWY1OWVkNTVlNzM3Yzc3MTQ3Y2Y1NWFkMGMxYjAzMGI2ZDdlZTc0OGE3NDI2OTUyZjliODUyZDVhOTM1ZTXSOzw9PlokY2xhc3NuYW1lWCRjbGFzc2VzXUdGUmVuZGVyU3RhdGWiP0BdR0ZSZW5kZXJTdGF0ZVhOU09iamVjdAAIABEAGgAkACkAMgA3AEkATABRAFMAWgBgAJsApACtAMkA5gD8AQ8BHgEpATQBTAFpAYIBnQGvAcMB0QHgAecB/gITAhoCNQI5Ak4CZwKBAoYCnAKdAp8CoQKjAqwCrgKwArICtAK2ArgCwQLDAsUCxgLOAxEDFgMhAyoDOAM7A0kAAAAAAAACAQAAAAAAAABBAAAAAAAAAAAAAAAAAAADUg==" options:0];
    GFRenderState *old=[NSKeyedUnarchiver unarchivedObjectOfClass:GFRenderState.class fromData:legacy error:nil];
    CHECK(old && old.schemaVersion==5 && old.uniformDomain==0 && old.uniformSourceTiming.length==0 && old.parameters.fov==1.7);
    printf("%lu checks passed, ProRes/HEVC timing regressions, clocks, bounds, metadata validation, state and legacy persistence\n",(unsigned long)checks);
    return 0;
}}
