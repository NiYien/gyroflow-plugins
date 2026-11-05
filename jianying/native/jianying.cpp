// SPDX-License-Identifier: GPL-3.0-or-later
// The shared OpenFX library performs stabilization; this adapter owns the host bridge.
#define NOMINMAX
#include <windows.h>
#include <shlobj.h>
#include <ofxImageEffect.h>
#include <ofxParam.h>
#include <ofxProperty.h>
#include <ofxOpenGLRender.h>
#include "angle_io.h"
#include "angle_directx_io.h"
#include "jianying_source_path.h"
#include <share.h>
#include <memory>
#include <chrono>
#include <algorithm>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <map>
#include <mutex>
#include <set>
#include <string>

namespace fs = std::filesystem;
static OfxPlugin* inner;
static OfxHost* host;
static OfxHost forwarded_host;
static const OfxPropertySuiteV1* prop;
static const OfxParameterSuiteV1* param;
static const OfxImageEffectSuiteV1* image;
static const OfxImageEffectOpenGLRenderSuiteV1* gpu_suite;
static OfxImageEffectSuiteV1 image_proxy;
static OfxPropertySuiteV1 property_proxy;
static fs::path output_dir;
static std::mutex log_mutex;
static FILE* log_file=nullptr;
static std::atomic<unsigned> sequence{0};
static thread_local unsigned frame_seq;
static thread_local int frame_mode;
static thread_local double frame_time;
static thread_local bool in_render;
static thread_local bool cpu_images;
static thread_local bool capture;
static thread_local const void* frame_instance;
static thread_local std::map<OfxPropertySetHandle, std::string> acquired;
static thread_local std::map<OfxImageClipHandle, std::string> clip_names;
static std::mutex capture_mutex;
static std::set<std::string> captured_buckets;

static bool verify_every_frame() {
    static const bool enabled=[] {const char* v=std::getenv("NIYIEN_OFX_VERIFY_ALL");return v && !std::strcmp(v,"1");}();
    return enabled;
}
static bool diagnostics_enabled() {
    static const bool enabled=[] {const char* v=std::getenv("NIYIEN_OFX_DIAGNOSTICS");return v && !std::strcmp(v,"1");}();
    return enabled;
}
using FrameClock=std::chrono::steady_clock;
static double elapsed_ms(FrameClock::time_point start) {return std::chrono::duration<double,std::milli>(FrameClock::now()-start).count();}
struct CpuBuffers {std::vector<unsigned char> source,output,readback;};
static thread_local CpuBuffers cached_cpu_buffers;

struct TextureView {
    OfxPropertySetHandle texture_handle=nullptr;
    unsigned texture=0;
    int bounds[4]{};
    std::vector<unsigned char> bytes;
    std::unique_ptr<AngleD3DTexture> directx;
    OfxPropertySetHandle handle() { return reinterpret_cast<OfxPropertySetHandle>(this); }
    int width() const { return bounds[2]-bounds[0]; }
    int height() const { return bounds[3]-bounds[1]; }
};
struct TextureFrame {
    TextureView source,output;
    OfxImageClipHandle source_clip=nullptr,output_clip=nullptr;
    AngleIO* gl=nullptr;
    int output_framebuffer=0;
    double source_ms=0;
    std::unique_ptr<AngleDirectX> directx;
};
static thread_local TextureFrame* texture_frame=nullptr;
static TextureView* texture_view(OfxPropertySetHandle p) {
    if(!texture_frame) return nullptr;
    if(p==texture_frame->source.handle()) return &texture_frame->source;
    if(p==texture_frame->output.handle()) return &texture_frame->output;
    return nullptr;
}
static OfxPropertySetHandle real_property_set(OfxPropertySetHandle p) {
    auto* v=texture_view(p);return v ? v->texture_handle : p;
}

static bool load_texture_view(TextureView& view,OfxImageClipHandle clip,double time) {
    if(view.texture_handle) { gpu_suite->clipFreeTexture(view.texture_handle);view.texture_handle=nullptr; }
    if(gpu_suite->clipLoadTexture(clip,time,kOfxBitDepthByte,nullptr,&view.texture_handle)!=kOfxStatOK) return false;
    int index=0,target=0;
    if(prop->propGetInt(view.texture_handle,kOfxImageEffectPropOpenGLTextureIndex,0,&index)!=0 ||
       prop->propGetInt(view.texture_handle,kOfxImageEffectPropOpenGLTextureTarget,0,&target)!=0 ||
       prop->propGetIntN(view.texture_handle,kOfxImagePropBounds,4,view.bounds)!=0) return false;
    view.texture=static_cast<unsigned>(index);
    return target==0x0DE1 && index>0 && view.width()>0 && view.height()>0;
}

template<class... T> static void report(const char* format, T... args) {
    if(output_dir.empty()) return;
    std::lock_guard<std::mutex> guard(log_mutex);
    if(!log_file) {
        log_file=_wfsopen((output_dir/L"events.log").c_str(),L"ab",_SH_DENYNO);
        if(!log_file) return;
        std::setvbuf(log_file,nullptr,_IOFBF,65536);
    }
    std::fprintf(log_file, "tick=%llu tid=%lu seq=%u instance=%p ", GetTickCount64(), GetCurrentThreadId(), frame_seq, frame_instance);
    std::fprintf(log_file, format, args...);
    std::fputc('\n', log_file);
}
static void flush_log() {std::lock_guard<std::mutex> guard(log_mutex);if(log_file) std::fflush(log_file);}

static void string_prop(const char* scope, OfxPropertySetHandle p, const char* key) {
    const char* value = nullptr;
    int status = prop->propGetString(p, key, 0, &value);
    report("property scope=%s key=%s status=%d value=%.1024s", scope, key, status, value ? value : "");
    if (status != kOfxStatOK || !value || !*value) return;
    std::string text(value);
    if (text.size() > 3 && (text[1] == ':' || text.rfind("file:", 0) == 0)) {
        // Keep URI candidates visible without guessing URL decoding rules.
        if (text.rfind("file:", 0) == 0) return;
        auto path = fs::u8path(text);
        path.replace_extension(L".gyroflow");
        std::error_code ec;
        bool exists = fs::is_regular_file(path, ec);
        report("companion scope=%s key=%s path=%s exists=%d error=%d", scope, key, path.u8string().c_str(), exists, ec.value());
    }
}

static void number_prop(const char* scope, OfxPropertySetHandle p, const char* key, int count = 1) {
    double v[4]{};
    int status = prop->propGetDoubleN(p, key, count, v);
    report("property scope=%s key=%s status=%d values=%.9g,%.9g,%.9g,%.9g", scope, key, status, v[0], v[1], v[2], v[3]);
}

static void path_properties(const char* scope, OfxPropertySetHandle p) {
    // These are optional host extensions. Missing properties are evidence, not errors to repair.
    for (auto key : {"OfxImageEffectPropSrcFilePath", "OfxImageEffectPropSourceFilePath", "OfxImageClipPropFilePath", "OfxImageClipPropFileName", "OfxImageEffectPropFilePath", "OfxImagePropFilePath", "OfxPropName"})
        string_prop(scope, p, key);
}

static unsigned long long hash_text(const char* s) {
    unsigned long long hash = 14695981039346656037ULL;
    for (; s && *s; ++s) { hash ^= static_cast<unsigned char>(*s); hash *= 1099511628211ULL; }
    return hash;
}

static void snapshot(OfxImageEffectHandle effect, double time) {
    OfxPropertySetHandle props = nullptr;
    if (image->getPropertySet(effect, &props) == kOfxStatOK) {
        path_properties("effect", props);
        number_prop("effect", props, kOfxImageEffectPropFrameRate);
        number_prop("effect", props, kOfxImageEffectPropProjectSize, 2);
    }
    for (auto name : {kOfxImageEffectSimpleSourceClipName, kOfxImageEffectOutputClipName}) {
        OfxImageClipHandle clip = nullptr;
        if (image->clipGetHandle(effect, name, &clip, &props) != kOfxStatOK) continue;
        clip_names[clip] = name;
        path_properties(name, props);
        for (auto key : {kOfxImageEffectPropFrameRate, kOfxImageEffectPropUnmappedFrameRate, kOfxImagePropPixelAspectRatio}) number_prop(name, props, key);
        number_prop(name, props, kOfxImageEffectPropFrameRange, 2);
        number_prop(name, props, kOfxImageEffectPropUnmappedFrameRange, 2);
        OfxRectD rod{};
        int status = image->clipGetRegionOfDefinition(clip, time, &rod);
        report("clip_rod name=%s time=%.9g status=%d rect=%.9g,%.9g,%.9g,%.9g", name, time, status, rod.x1, rod.y1, rod.x2, rod.y2);
    }
    OfxParamSetHandle params = nullptr;
    if (image->getParamSet(effect, &params) != kOfxStatOK) return;
    for (auto name : {"ProjectPath", "ProjectData", "InstanceId", "EmbeddedLensProfile", "EmbeddedPreset", "Status"}) {
        OfxParamHandle handle = nullptr;
        int status = param->paramGetHandle(params, name, &handle, nullptr);
        if (status != kOfxStatOK) { report("param name=%s handle_status=%d", name, status); continue; }
        char* value = nullptr;
        int a = param->paramGetValue(handle, &value);
        // Host-owned string pointers may be invalidated by the next getter.
        std::string ui = a == kOfxStatOK && value ? value : "";
        value = nullptr;
        int b = param->paramGetValueAtTime(handle, time, &value);
        std::string evaluated = b == kOfxStatOK && value ? value : "";
        report("param name=%s time=%.9g ui_status=%d ui_len=%zu ui_hash=%llx eval_status=%d eval_len=%zu eval_hash=%llx equal=%d", name, time, a, ui.size(), hash_text(ui.c_str()), b, evaluated.size(), hash_text(evaluated.c_str()), ui == evaluated);
        if (!std::strcmp(name, "ProjectPath")) {
            report("project_path ui=%s evaluated=%s", ui.c_str(), evaluated.c_str());
            for (const auto& path : {ui, evaluated}) if (!path.empty()) {
                std::error_code ec;
                bool exists = fs::is_regular_file(fs::u8path(path), ec);
                report("project_exists path=%s exists=%d error=%d", path.c_str(), exists, ec.value());
            }
        }
    }
    for (auto name : {"IncludeProjectData", "DisableStretch", "DontDrawOutside", "DiagnosticMode", "InputRotation", "Interpolation", "IntegrationMethod", "ZoomMode", "UseGyroflowsKeyframes", "FlipHorizontal", "FlipVertical", "TranslationEnabled"}) {
        OfxParamHandle handle = nullptr;
        if (param->paramGetHandle(params, name, &handle, nullptr) != kOfxStatOK) continue;
        int ui = -999, value = -999;
        int a = param->paramGetValue(handle, &ui);
        int b = param->paramGetValueAtTime(handle, time, &value);
        report("param_int name=%s ui_status=%d ui=%d eval_status=%d evaluated=%d", name, a, ui, b, value);
    }
    for (auto name : {"Fov", "Smoothness", "VideoSpeed", "Rotation", "OutputWidth", "OutputHeight", "OutputZoom", "OutputRotation", "OutputOffsetX", "OutputOffsetY", "LensCorrectionStrength", "ZoomLimit", "HorizonLockAmount", "HorizonLockRoll", "TranslationReference", "TranslationSmoothness"}) {
        OfxParamHandle handle = nullptr;
        if (param->paramGetHandle(params, name, &handle, nullptr) != kOfxStatOK) continue;
        double ui = -999, value = -999;
        int a = param->paramGetValue(handle, &ui);
        int b = param->paramGetValueAtTime(handle, time, &value);
        report("param_double name=%s ui_status=%d ui=%.9g eval_status=%d evaluated=%.9g", name, a, ui, b, value);
    }
}

struct Pixels {
    unsigned char* data = nullptr;
    int bounds[4]{}, stride = 0;
    int width = 0, height = 0;
};

static bool pixels(OfxPropertySetHandle p, Pixels& v) {
    if(auto* view=texture_view(p)) {
        v.data=view->bytes.data();std::copy(view->bounds,view->bounds+4,v.bounds);
        v.width=view->width();v.height=view->height();v.stride=v.width*4;return true;
    }
    const char *depth = nullptr, *components = nullptr;
    if (prop->propGetPointer(p, kOfxImagePropData, 0, reinterpret_cast<void**>(&v.data)) != kOfxStatOK ||
        prop->propGetIntN(p, kOfxImagePropBounds, 4, v.bounds) != kOfxStatOK ||
        prop->propGetInt(p, kOfxImagePropRowBytes, 0, &v.stride) != kOfxStatOK ||
        prop->propGetString(p, kOfxImageEffectPropPixelDepth, 0, &depth) != kOfxStatOK ||
        prop->propGetString(p, kOfxImageEffectPropComponents, 0, &components) != kOfxStatOK) return false;
    v.width = v.bounds[2] - v.bounds[0]; v.height = v.bounds[3] - v.bounds[1];
    // The adapter advertises byte RGBA only; never interpret a GPU handle as CPU memory.
    return v.data && depth && components && !std::strcmp(depth, kOfxBitDepthByte) && !std::strcmp(components, kOfxImageComponentRGBA) &&
        v.width > 0 && v.height > 0 && std::abs(static_cast<long long>(v.stride)) >= static_cast<long long>(v.width) * 4;
}

static void capture_image(OfxPropertySetHandle p, const char* role) {
    if(!capture && !verify_every_frame()) return;
    if (!cpu_images) { report("capture_skipped_gpu role=%s", role); return; }
    Pixels v;
    if (!pixels(p, v)) { report("capture_unsupported role=%s", role); return; }
    unsigned long long hash = 14695981039346656037ULL;
    double sum[4]{};
    unsigned minv[4]{255,255,255,255}, maxv[4]{};
    unsigned count = 0;
    for (int y = 0; y < v.height; y += std::max(1, v.height / 48)) {
        const auto* row = v.data + static_cast<ptrdiff_t>(y) * v.stride;
        for (int x = 0; x < v.width; x += std::max(1, v.width / 64)) {
            for (int c = 0; c < 4; ++c) {
                unsigned n = row[x*4+c]; sum[c] += n; minv[c] = std::min(minv[c], n); maxv[c] = std::max(maxv[c], n);
                hash ^= n; hash *= 1099511628211ULL;
            }
            ++count;
        }
    }
    report("pixels role=%s mode=%d time=%.9g size=%dx%d stride=%d ptr=%p grid_hash=%llx mean=%.3f,%.3f,%.3f,%.3f min=%u,%u,%u,%u max=%u,%u,%u,%u", role, frame_mode, frame_time, v.width, v.height, v.stride, v.data, hash, sum[0]/count,sum[1]/count,sum[2]/count,sum[3]/count,minv[0],minv[1],minv[2],minv[3],maxv[0],maxv[1],maxv[2],maxv[3]);
    if (!capture) return;
    char name[128]; std::snprintf(name, sizeof(name), "%06u-mode%d-%s.ppm", frame_seq, frame_mode, role);
    FILE* f = nullptr;
    if (_wfopen_s(&f, (output_dir / name).c_str(), L"wb") != 0) return;
    int w = std::min(480, v.width), h = std::max(1, v.height * w / v.width);
    std::fprintf(f, "P6\n%d %d\n255\n", w, h);
    for (int y = 0; y < h; ++y) {
        const auto* row = v.data + static_cast<ptrdiff_t>(y * v.height / h) * v.stride;
        for (int x = 0; x < w; ++x) std::fwrite(row + (x * v.width / w) * 4, 1, 3, f);
    }
    std::fclose(f);
}

static OfxStatus get_image(OfxImageClipHandle clip, OfxTime time, const OfxRectD* region, OfxPropertySetHandle* result) {
    if(texture_frame) {
        TextureView* view=nullptr;
        if(clip==texture_frame->output_clip) view=&texture_frame->output;
        else if(clip==texture_frame->source_clip) {
            const auto started=FrameClock::now();
            view=&texture_frame->source;
            if(!load_texture_view(*view,clip,time)) return kOfxStatFailed;
            try {
                if(texture_frame->directx) {
                    view->directx=std::make_unique<AngleD3DTexture>(*texture_frame->directx,*texture_frame->gl,view->width(),view->height());
                    view->directx->copy_from(view->texture);
                } else texture_frame->gl->read(view->texture,view->width(),view->height(),view->bytes);
            }
            catch(const std::exception& e) { report("angle_source_failed reason=%s",e.what());return kOfxStatFailed; }
            texture_frame->source_ms+=elapsed_ms(started);
        }
        if(!view) return kOfxStatErrBadHandle;
        *result=view->handle();acquired[*result]=clip_names[clip];
        if(capture) report("texture_cpu_view role=%s time=%.9g texture=%u size=%dx%d",clip_names[clip].c_str(),time,view->texture,view->width(),view->height());
        if(view==&texture_frame->source) capture_image(*result,"input");
        return kOfxStatOK;
    }
    int status = image->clipGetImage(clip, time, region, result);
    if(capture || status!=0) report("get_image clip=%p role=%s time=%.9g status=%d handle=%p", clip, clip_names[clip].c_str(), time, status, status == kOfxStatOK ? *result : nullptr);
    if (status == kOfxStatOK && in_render) {
        acquired[*result] = clip_names[clip];
        if (capture) {
            for (auto key : {kOfxImageEffectPropPixelDepth, kOfxImageEffectPropComponents, kOfxImageEffectPropPreMultiplication, kOfxImagePropUniqueIdentifier}) string_prop(clip_names[clip].c_str(), *result, key);
            int b[4]{}; int s = prop->propGetIntN(*result, kOfxImagePropBounds, 4, b);
            report("image_bounds status=%d rect=%d,%d,%d,%d", s,b[0],b[1],b[2],b[3]);
            s = prop->propGetIntN(*result, kOfxImagePropRegionOfDefinition, 4, b);
            report("image_rod status=%d rect=%d,%d,%d,%d", s,b[0],b[1],b[2],b[3]);
            number_prop("image", *result, kOfxImageEffectPropRenderScale, 2);
            path_properties("image", *result);
        }
        if (clip_names[clip] == "Source") capture_image(*result, "input");
    }
    return status;
}

static OfxStatus release_image(OfxPropertySetHandle p) {
    auto it = acquired.find(p);
    if (it != acquired.end() && it->second == "Output") capture_image(p, "output");
    int status = texture_view(p) ? kOfxStatOK : image->clipReleaseImage(p);
    if(capture || status!=0) report("release_image handle=%p status=%d", p, status);
    acquired.erase(p);
    return status;
}

static const void* fetch_suite(OfxPropertySetHandle h, const char* name, int version) {
    const void* suite = host->fetchSuite(h, name, version);
    if (!std::strcmp(name, kOfxImageEffectSuite) && version == 1 && suite) return &image_proxy;
    if (!std::strcmp(name, kOfxPropertySuite) && version == 1 && suite) return &property_proxy;
    return suite;
}

static void property_failure(const char* kind, const char* key, int status) {
    if (in_render && capture && status != kOfxStatOK) report("inner_property_failure kind=%s key=%s status=%d",kind,key,status);
}

static OfxStatus get_int(OfxPropertySetHandle p,const char* key,int index,int* value) {
    if(texture_frame && !std::strcmp(key,"NiYienImageEffectPropD3D11Enabled")) { *value=texture_frame->directx ? 1 : 0;return 0; }
    if(texture_frame && !std::strcmp(key,kOfxImageEffectPropOpenGLEnabled)) { *value=0;return 0; }
    if(auto* v=texture_view(p)) if(!std::strcmp(key,kOfxImagePropRowBytes)) { *value=v->width()*4;return 0; }
    int s=prop->propGetInt(real_property_set(p),key,index,value);property_failure("int",key,s);return s;
}
static OfxStatus get_int_n(OfxPropertySetHandle p,const char* key,int count,int* value) { int s=prop->propGetIntN(real_property_set(p),key,count,value);property_failure("intN",key,s);return s; }
static OfxStatus get_double(OfxPropertySetHandle p,const char* key,int index,double* value) { int s=prop->propGetDouble(real_property_set(p),key,index,value);property_failure("double",key,s);return s; }
static OfxStatus get_double_n(OfxPropertySetHandle p,const char* key,int count,double* value) { int s=prop->propGetDoubleN(real_property_set(p),key,count,value);property_failure("doubleN",key,s);return s; }
static OfxStatus get_string(OfxPropertySetHandle p,const char* key,int index,const char** value) {
    if (!std::strcmp(key,"NiYienJianyingSourcePath") && index == 0 && frame_instance) {
        OfxPropertySetHandle effect_properties = nullptr;
        auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(frame_instance));
        if (image->getPropertySet(effect,&effect_properties) == kOfxStatOK && p == effect_properties) {
            static thread_local std::string source_path;
            source_path = jianying_source_path::copy_source_path(frame_instance);
            if (source_path.empty()) return kOfxStatErrUnknown;
            *value = source_path.c_str();
            return kOfxStatOK;
        }
    }
    if(texture_view(p)) {
        if(!std::strcmp(key,kOfxImageEffectPropPixelDepth)) { *value=kOfxBitDepthByte;return 0; }
        if(!std::strcmp(key,kOfxImageEffectPropComponents)) { *value=kOfxImageComponentRGBA;return 0; }
    }
    int s=prop->propGetString(real_property_set(p),key,index,value);property_failure("string",key,s);return s;
}
static OfxStatus get_pointer(OfxPropertySetHandle p,const char* key,int index,void** value) {
    if(auto* v=texture_view(p)) if(v->directx) {
        if(!std::strcmp(key,"NiYienImagePropD3D11Texture")) { *value=v->directx->texture.Get();return 0; }
        if(!std::strcmp(key,"NiYienImagePropD3D11Device")) { *value=texture_frame->directx->device.Get();return 0; }
        if(!std::strcmp(key,"NiYienImagePropD3D11DeviceContext")) { *value=texture_frame->directx->context.Get();return 0; }
    }
    if(auto* v=texture_view(p)) if(!std::strcmp(key,kOfxImagePropData)) { *value=v->bytes.data();return 0; }
    int s=prop->propGetPointer(real_property_set(p),key,index,value);property_failure("pointer",key,s);return s;
}

static void set_host(OfxHost* h) {
    host = h;
    prop = static_cast<const OfxPropertySuiteV1*>(h->fetchSuite(h->host, kOfxPropertySuite, 1));
    param = static_cast<const OfxParameterSuiteV1*>(h->fetchSuite(h->host, kOfxParameterSuite, 1));
    image = static_cast<const OfxImageEffectSuiteV1*>(h->fetchSuite(h->host, kOfxImageEffectSuite, 1));
    gpu_suite=static_cast<const OfxImageEffectOpenGLRenderSuiteV1*>(h->fetchSuite(h->host,kOfxOpenGLRenderSuite,1));
    if (prop) {
        property_proxy=*prop;
        property_proxy.propGetInt=get_int; property_proxy.propGetIntN=get_int_n;
        property_proxy.propGetDouble=get_double; property_proxy.propGetDoubleN=get_double_n;
        property_proxy.propGetString=get_string; property_proxy.propGetPointer=get_pointer;
    }
    if (image) { image_proxy = *image; image_proxy.clipGetImage = get_image; image_proxy.clipReleaseImage = release_image; }
    forwarded_host = *h; forwarded_host.fetchSuite = fetch_suite;
    inner->setHost(&forwarded_host);
}

static OfxStatus diagnostic_render(OfxImageEffectHandle effect, OfxPropertySetHandle in, double time, int mode) {
    if (!cpu_images) return kOfxStatErrUnsupported;
    OfxImageClipHandle src_clip = nullptr, dst_clip = nullptr;
    OfxPropertySetHandle src = nullptr, dst = nullptr;
    if (image->clipGetHandle(effect, "Source", &src_clip, nullptr) != kOfxStatOK || image->clipGetHandle(effect, "Output", &dst_clip, nullptr) != kOfxStatOK) return kOfxStatFailed;
    clip_names[src_clip] = "Source"; clip_names[dst_clip] = "Output";
    int status = get_image(src_clip, time, nullptr, &src);
    if (status != kOfxStatOK) return status;
    status = get_image(dst_clip, time, nullptr, &dst);
    if (status != kOfxStatOK) { release_image(src); return status; }
    Pixels a,b; int window[4]{};
    bool valid = pixels(src,a) && pixels(dst,b) && prop->propGetIntN(in,kOfxImageEffectPropRenderWindow,4,window) == kOfxStatOK;
    if (valid) valid = window[0] >= b.bounds[0] && window[1] >= b.bounds[1] && window[2] <= b.bounds[2] && window[3] <= b.bounds[3] && window[0] < window[2] && window[1] < window[3];
    if (valid && mode != 2) valid = window[0] >= a.bounds[0] && window[1] >= a.bounds[1] && window[2] <= a.bounds[2] && window[3] <= a.bounds[3];
    if (valid) for (int y = window[1]; y < window[3]; ++y) {
        auto* out = b.data + static_cast<ptrdiff_t>(y-b.bounds[1])*b.stride;
        if (mode != 2) {
            const auto* input = a.data + static_cast<ptrdiff_t>(y-a.bounds[1])*a.stride;
            std::memcpy(out+(window[0]-b.bounds[0])*4,input+(window[0]-a.bounds[0])*4,static_cast<size_t>(window[2]-window[0])*4);
            if (mode == 3) for (int x=window[0];x<window[2];++x) out[(x-b.bounds[0])*4+3]=255;
        } else for (int x = window[0]; x < window[2]; ++x) {
            auto* p = out+(x-b.bounds[0])*4;
            p[0] = (x-b.bounds[0]) < b.width/2 ? 255 : 0;
            p[1] = (y-b.bounds[1]) < b.height/2 ? 255 : 0;
            p[2] = (x-b.bounds[0]) >= b.width/2 ? 255 : 0; p[3] = 255;
        }
    }
    release_image(dst); release_image(src);
    return valid ? kOfxStatOK : kOfxStatErrFormat;
}

static int sweep_mode(double time, double fps) {
    const int modes[]={1,2,3,0};
    int phase=static_cast<int>(std::floor(std::max(0.0,time)/std::max(1.0,fps)/2.5))%4;
    return modes[phase];
}

static OfxStatus texture_render(OfxImageEffectHandle effect,OfxPropertySetHandle in,double time,int mode) {
    if(!gpu_suite) { report("angle_bridge_missing_suite");return kOfxStatErrMissingHostFeature; }
    TextureFrame frame;int status=kOfxStatFailed;
    const auto total_start=FrameClock::now();
    frame.source.bytes.swap(cached_cpu_buffers.source);frame.output.bytes.swap(cached_cpu_buffers.output);
    std::vector<unsigned char> readback;readback.swap(cached_cpu_buffers.readback);
    double inner_ms=0,write_ms=0,verify_ms=0;
    const bool verify_output=verify_every_frame() || (diagnostics_enabled() && (capture || frame_seq%30==1));
    try {
        static AngleIO gl;frame.gl=&gl;
        const auto* version=gl.getString(0x1F02);
        if(!version) throw std::runtime_error("No current host ANGLE context");
        if(image->clipGetHandle(effect,"Source",&frame.source_clip,nullptr)!=0 || image->clipGetHandle(effect,"Output",&frame.output_clip,nullptr)!=0) throw std::runtime_error("Missing host clips");
        if(!load_texture_view(frame.output,frame.output_clip,time)) throw std::runtime_error("Cannot load host output texture");
        gl.getInt(0x8CA6,&frame.output_framebuffer);
        int attachment_type=0,attachment_name=0;
        if(capture) {
            gl.getFramebufferAttachment(0x8CA9,0x8CE0,0x8CD0,&attachment_type);
            gl.getFramebufferAttachment(0x8CA9,0x8CE0,0x8CD1,&attachment_name);
        }
        unsigned incoming_error=gl.getError();
        if(capture || incoming_error) report("host_output framebuffer=%d attachment_type=%d attachment_name=%d metadata_texture=%u error=%u",frame.output_framebuffer,attachment_type,attachment_name,frame.output.texture,incoming_error);
        // Preserve the original transparent initialization, including pixels
        // outside any core output region, while reusing the allocation.
        const char* directx_setting=std::getenv("NIYIEN_OFX_DIRECTX");
        if(mode==0 && !(directx_setting && !std::strcmp(directx_setting,"0"))) {
            frame.directx=std::make_unique<AngleDirectX>();
            frame.output.directx=std::make_unique<AngleD3DTexture>(*frame.directx,gl,frame.output.width(),frame.output.height());
            frame.output.directx->clear();
        } else frame.output.bytes.assign(static_cast<size_t>(frame.output.width())*frame.output.height()*4,0);
        if(capture) report("angle_bridge_begin version=%s output_texture=%u size=%dx%d",version,frame.output.texture,frame.output.width(),frame.output.height());
        texture_frame=&frame;cpu_images=!frame.directx;
        const auto inner_start=FrameClock::now();
        status=mode ? diagnostic_render(effect,in,time,mode) : inner->mainEntry(kOfxImageEffectActionRender,effect,in,nullptr);
        inner_ms=elapsed_ms(inner_start);
        texture_frame=nullptr;
        if(status==kOfxStatOK) {
            const auto write_start=FrameClock::now();
            if(frame.directx) frame.output.directx->copy_to(frame.output_framebuffer);
            else gl.write_framebuffer(frame.output_framebuffer,frame.output.width(),frame.output.height(),frame.output.bytes);
            write_ms=elapsed_ms(write_start);
            if(verify_output) {
                const auto verify_start=FrameClock::now();
                if(frame.directx) frame.output.directx->read(frame.output.bytes);
                gl.read_framebuffer(frame.output_framebuffer,frame.output.width(),frame.output.height(),readback);
                bool equal=readback==frame.output.bytes;
                report("angle_bridge_uploaded framebuffer=%d readback_equal=%d",frame.output_framebuffer,equal);
                frame.output.bytes.swap(readback);texture_frame=&frame;
                capture_image(frame.output.handle(),"readback");texture_frame=nullptr;
                if(!equal) status=kOfxStatFailed;
                verify_ms=elapsed_ms(verify_start);
            }
        }
    } catch(const std::exception& e) { report("angle_bridge_failed reason=%s",e.what());status=kOfxStatFailed; }
    texture_frame=nullptr;
    if(frame.source.texture_handle) {int s=gpu_suite->clipFreeTexture(frame.source.texture_handle);if(capture || s) report("free_source_texture status=%d",s);}
    if(frame.output.texture_handle) {int s=gpu_suite->clipFreeTexture(frame.output.texture_handle);if(capture || s) report("free_output_texture status=%d",s);}
    const bool used_directx=bool(frame.directx);
    frame.source.directx.reset();frame.output.directx.reset();frame.directx.reset();
    report("frame_timing mode=%d directx=%d status=%d source_ms=%.3f core_and_params_ms=%.3f write_ms=%.3f verify_ms=%.3f verified=%d total_ms=%.3f",mode,used_directx,status,frame.source_ms,inner_ms-frame.source_ms,write_ms,verify_ms,verify_output,elapsed_ms(total_start));
    frame.source.bytes.swap(cached_cpu_buffers.source);frame.output.bytes.swap(cached_cpu_buffers.output);readback.swap(cached_cpu_buffers.readback);
    return status;
}

static bool snapshot_parameter(const char* name) {
    for(auto id:{"Browse","ProjectPath","ProjectData","ReloadProject","IncludeProjectData","DiagnosticMode"}) if(name && !std::strcmp(name,id)) return true;
    return false;
}

static OfxStatus entry(const char* action, const void* handle, OfxPropertySetHandle in, OfxPropertySetHandle out) {
    if (!inner || !image || !prop || !param) return kOfxStatErrMissingHostFeature;
    auto effect = reinterpret_cast<OfxImageEffectHandle>(const_cast<void*>(handle));
    frame_instance = handle;
    bool snapshot_change=false;
    if(!std::strcmp(action,kOfxActionInstanceChanged)) {
        const char* name=nullptr;
        prop->propGetString(in,kOfxPropName,0,&name);
        snapshot_change=snapshot_parameter(name);
    }
    if(!std::strcmp(action,"OfxActionOpenGLContextAttached") || !std::strcmp(action,"OfxActionOpenGLContextDetached") || !std::strcmp(action,"kOfxActionOpenGLContextDetached")) {
        report("host_gl_context action=%s",action);return kOfxStatOK;
    }
    bool render = !std::strcmp(action,kOfxImageEffectActionRender);
    double time = 0; if (in) prop->propGetDouble(in,kOfxPropTime,0,&time);
    int mode = 0;
    if (render || !std::strcmp(action,kOfxImageEffectActionIsIdentity)) {
        OfxParamSetHandle params = nullptr; OfxParamHandle p = nullptr;
        if (image->getParamSet(effect,&params) == kOfxStatOK && param->paramGetHandle(params,"DiagnosticMode",&p,nullptr) == kOfxStatOK) param->paramGetValueAtTime(p,time,&mode);
    }
    if (!std::strcmp(action,kOfxImageEffectActionIsIdentity) && mode != 0) return kOfxStatReplyDefault;
    if (diagnostics_enabled() && !std::strcmp(action,kOfxImageEffectActionIsIdentity)) {
        char key[100]; std::snprintf(key,sizeof(key),"identity-%p-%lld",handle,static_cast<long long>(std::floor(time/30.0)));
        bool take=false;
        { std::lock_guard<std::mutex> guard(capture_mutex); take=captured_buckets.size()<150 && captured_buckets.insert(key).second; }
        if(take) snapshot(effect,time);
    }
    if (render) {
        in_render = true; frame_seq = ++sequence; frame_mode = mode; frame_time = time;
        for (auto name : {"Source","Output"}) {
            OfxImageClipHandle clip=nullptr;
            if(image->clipGetHandle(effect,name,&clip,nullptr)==kOfxStatOK) clip_names[clip]=name;
        }
        cpu_images = true;
        for (auto key : {"OfxImageEffectPropOpenGLEnabled","OfxImageEffectPropCudaEnabled","OfxImageEffectPropOpenCLEnabled","OfxImageEffectPropMetalEnabled","OfxImageEffectPropAngleEnabled"}) { int v=0; if (prop->propGetInt(in,key,0,&v)==kOfxStatOK && v) cpu_images=false; }
        OfxPropertySetHandle props = nullptr; double fps = 30;
        if (image->getPropertySet(effect,&props) == kOfxStatOK) prop->propGetDouble(props,kOfxImageEffectPropFrameRate,0,&fps);
        if (mode == 4) {
            mode=sweep_mode(time,fps);frame_mode=mode;
            report("sweep effective_mode=%d time=%.9g fps=%.9g",mode,time,fps);
        }
        char key[100]; std::snprintf(key,sizeof(key),"%p-%d-%lld",handle,mode,static_cast<long long>(std::floor(time / std::max(1.0,fps))));
        capture=false;
        if(diagnostics_enabled()) { std::lock_guard<std::mutex> guard(capture_mutex); capture = captured_buckets.size() < 150 && captured_buckets.insert(key).second; }
        if (capture) {
            snapshot(effect,time);
            number_prop("render",in,kOfxImageEffectPropRenderScale,2);
            int w[4]{}; int s = prop->propGetIntN(in,kOfxImageEffectPropRenderWindow,4,w);
            report("render_window status=%d rect=%d,%d,%d,%d",s,w[0],w[1],w[2],w[3]);
            for (auto k : {"OfxImageEffectPropOpenGLEnabled","OfxImageEffectPropCudaEnabled","OfxImageEffectPropOpenCLEnabled","OfxImageEffectPropMetalEnabled","OfxImageEffectPropAngleEnabled","OfxImageEffectPropSourceFrame"}) { int n=-1; int s=prop->propGetInt(in,k,0,&n); report("render_flag key=%s status=%d value=%d",k,s,n); }
        }
    }
    bool trace_action=render || snapshot_change || !std::strcmp(action,kOfxActionCreateInstance) || !std::strcmp(action,kOfxActionDestroyInstance) || !std::strcmp(action,kOfxActionLoad) || !std::strcmp(action,kOfxActionUnload) || !std::strcmp(action,kOfxActionDescribe) || !std::strcmp(action,kOfxImageEffectActionDescribeInContext);
    if(trace_action) report("action_begin action=%s time=%.9g mode=%d",action,time,mode);
    if(render) flush_log();
    auto start = GetTickCount64();
    int host_gl=0;if(render) prop->propGetInt(in,kOfxImageEffectPropOpenGLEnabled,0,&host_gl);
    OfxStatus status = render && host_gl ? texture_render(effect,in,time,mode) : render && mode != 0 ? diagnostic_render(effect,in,time,mode) : inner->mainEntry(action,handle,in,out);
    if (!std::strcmp(action,kOfxActionDescribe) && status == kOfxStatOK) {
        OfxPropertySetHandle props = nullptr; image->getPropertySet(effect,&props);
        for (auto key : {kOfxPropLabel,kOfxPropShortLabel,kOfxPropLongLabel}) prop->propSetString(props,key,0,"Gyroflow 剪映");
        const char* depth[] = {kOfxBitDepthByte}; prop->propSetStringN(props,kOfxImageEffectPropSupportedPixelDepths,1,depth);
        for (auto key : {"OfxImageEffectPropOpenGLRenderSupported","OfxImageEffectPropOpenCLRenderSupported","OfxImageEffectPropCudaRenderSupported","OfxImageEffectPropMetalRenderSupported","OfxImageEffectPropAngleRenderSupported"}) prop->propSetString(props,key,0,"false");
        // Jianying prefers desktop OpenGL over ANGLE when both are advertised.
        prop->propSetString(props,"OfxImageEffectPropAngleRenderSupported",0,"true");
        report("gpu_negotiation desktop_opengl=false angle=true");
        path_properties("host",host->host);
        for (auto key : {kOfxPropVersionLabel,"OfxImageEffectPropOpenGLRenderSupported","OfxImageEffectPropAngleRenderSupported"}) string_prop("host",host->host,key);
        report("environment NO_OPENCL=%s NO_WGPU=%s",std::getenv("NO_OPENCL") ? std::getenv("NO_OPENCL") : "",std::getenv("NO_WGPU") ? std::getenv("NO_WGPU") : "");
    }
    if (!std::strcmp(action,kOfxImageEffectActionDescribeInContext) && status == kOfxStatOK) {
        OfxParamSetHandle params; OfxPropertySetHandle p;
        image->getParamSet(effect,&params);
        int s = param->paramDefine(params,kOfxParamTypeChoice,"DiagnosticMode",&p);
        if (s == kOfxStatOK) {
            prop->propSetString(p,kOfxPropLabel,0,"Diagnostic mode");
            prop->propSetString(p,kOfxParamPropHint,0,"Internal rendering diagnostics.");
            prop->propSetInt(p,kOfxParamPropDefault,0,0);
            prop->propSetInt(p,kOfxParamPropAnimates,0,0);
            prop->propSetInt(p,kOfxParamPropSecret,0,1);
            const char* choices[] = {"0 - Stabilize","1 - Source copy","2 - Color pattern","3 - Source opaque","4 - Batch sweep (2.5s each)"}; prop->propSetStringN(p,kOfxParamPropChoiceOption,5,choices);
            OfxParamHandle page; OfxPropertySetHandle page_props;
            if (param->paramGetHandle(params,"Main",&page,&page_props) == kOfxStatOK) { int count=0; prop->propGetDimension(page_props,kOfxParamPropPageChild,&count); prop->propSetString(page_props,kOfxParamPropPageChild,count,"DiagnosticMode"); }
        }
        report("define_diagnostic_mode status=%d",s);
    }
    if (!std::strcmp(action,kOfxActionCreateInstance) || snapshot_change) {
        if (in) { string_prop("change",in,kOfxPropName); string_prop("change",in,kOfxPropChangeReason); }
        snapshot(effect,time);
    }
    if(trace_action) report("action_end action=%s status=%d elapsed_ms=%llu unreturned_images=%zu",action,status,GetTickCount64()-start,acquired.size());
    if(trace_action) flush_log();
    if (render) { in_render=false; capture=false; acquired.clear(); }
    if(!std::strcmp(action,kOfxActionUnload)) {
        std::lock_guard<std::mutex> guard(log_mutex);
        if(log_file) {std::fclose(log_file);log_file=nullptr;}
    }
    return status;
}

static OfxPlugin plugin = {kOfxImageEffectPluginApi,1,"xyz.niyien.gyroflow.jianying.batch11",1,11,set_host,entry};
extern "C" __declspec(dllexport) int OfxGetNumberOfPlugins() { return 1; }
extern "C" __declspec(dllexport) OfxPlugin* OfxGetPlugin(int index) {
    if (index != 0) return nullptr;
    static std::once_flag once;
    std::call_once(once, [] {
        HMODULE own = nullptr; wchar_t path[32768]{};
        GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,reinterpret_cast<LPCWSTR>(&OfxGetPlugin),&own);
        GetModuleFileNameW(own,path,32768);
        auto binary_dir = fs::path(path).parent_path();
        PWSTR local_app_data = nullptr;
        if (SUCCEEDED(SHGetKnownFolderPath(FOLDERID_LocalAppData, KF_FLAG_CREATE, nullptr, &local_app_data))) {
            output_dir = fs::path(local_app_data) / L"NiYien/GyroflowNiYien/plugins/jianying" / (L"pid-"+std::to_wstring(GetCurrentProcessId())+L"-"+std::to_wstring(GetTickCount64()));
            CoTaskMemFree(local_app_data);
            std::error_code ec; fs::create_directories(output_dir,ec);
        }
        HMODULE dll = LoadLibraryW((binary_dir / L"gyroflow_ofx.dll").c_str());
        if (!dll) { report("load_inner_failed error=%lu",GetLastError()); return; }
        auto get = reinterpret_cast<OfxPlugin*(*)(int)>(GetProcAddress(dll,"OfxGetPlugin"));
        if (get) inner = get(0);
        report("jianying_adapter_loaded inner=%s diagnostics=%d verify_every_frame=%d",inner ? inner->pluginIdentifier : "missing",diagnostics_enabled(),verify_every_frame());flush_log();
    });
    return inner ? &plugin : nullptr;
}
