#import <Foundation/Foundation.h>

static const UInt32 kGFProjectGroup = 1000;
static const UInt32 kGFProjectControl = 1001;
static const UInt32 kGFInstanceIdentity = 1901;
static const UInt32 kGFProjectPayload = 1902;
static const UInt32 kGFTimingPayload = 1903;
static const UInt32 kGFProjectPayloadManifestA = 1904;
static const UInt32 kGFProjectPayloadManifestB = 1905;
static const UInt32 kGFProjectDisplayName = 1906;

static const NSUInteger kGFProjectPayloadChunksPerBank = 10;
static const NSUInteger kGFProjectPayloadChunkBytes = 416 * 1024;
static const NSUInteger kGFProjectPayloadMaximumBytes = 4 * 1024 * 1024;

static const UInt32 kGFProjectPayloadChunksA[10] = {
    1910, 1911, 1912, 1913, 1914, 1915, 1916, 1917,
    1918, 1919,
};

static const UInt32 kGFProjectPayloadChunksB[10] = {
    1930, 1931, 1932, 1933, 1934, 1935, 1936, 1937,
    1938, 1939,
};

static const UInt32 kGFAdjustmentGroup = 2000;
static const UInt32 kGFFOV = 2001;
static const UInt32 kGFSmoothness = 2002;
static const UInt32 kGFLensCorrection = 2003;
static const UInt32 kGFHorizonLockAmount = 2004;
static const UInt32 kGFHorizonLockRoll = 2005;
static const UInt32 kGFZoomMode = 2006;
static const UInt32 kGFOverview = 2007;
