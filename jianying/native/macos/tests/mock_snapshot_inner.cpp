// SPDX-License-Identifier: GPL-3.0-or-later
// Snapshot diagnostics use this no-GPU inner implementation only.
#include <ofxCore.h>
#include <ofxImageEffect.h>
#include <ofxProperty.h>
#include <cstring>

namespace {
const OfxPropertySuiteV1* properties = nullptr;
unsigned calls = 0;
void set_host(OfxHost* host) {
    properties = static_cast<const OfxPropertySuiteV1*>(host->fetchSuite(host->host, kOfxPropertySuite, 1));
}
OfxStatus entry(const char*, const void*, OfxPropertySetHandle in, OfxPropertySetHandle) {
    ++calls;
    int status = kOfxStatOK;
    if (in) properties->propGetInt(in, "MockInnerStatus", 0, &status);
    return status;
}
OfxPlugin plugin{kOfxImageEffectPluginApi, 1, "test.snapshot-inner", 0, 1, set_host, entry};
}
extern "C" __attribute__((visibility("default"))) int OfxGetNumberOfPlugins() { return 1; }
extern "C" __attribute__((visibility("default"))) OfxPlugin* OfxGetPlugin(int index) { return index == 0 ? &plugin : nullptr; }
extern "C" __attribute__((visibility("default"))) unsigned MockSnapshotCalls() { return calls; }
