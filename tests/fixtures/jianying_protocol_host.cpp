// SPDX-License-Identifier: GPL-3.0-or-later
#include <windows.h>
#include <ofxCore.h>
#include <ofxProperty.h>
#include <ofxParam.h>
#include <ofxImageEffect.h>
#include <ofxMemory.h>
#include <ofxMultiThread.h>
#include <ofxMessage.h>
#include <ofxProgress.h>
#include <ofxTimeLine.h>
#include <algorithm>
#include <cstdarg>
#include <cstdlib>
#include <fstream>
#include <iomanip>
#include <map>
#include <memory>
#include <mutex>
#include <sstream>
#include <set>
#include <stdexcept>
#include <string>
#include <vector>

struct Effect;
struct Param;
struct Property {
    enum Kind { Integer, Double, String, Pointer } kind = Integer;
    std::vector<int> integers;
    std::vector<double> doubles;
    std::vector<std::string> strings;
    std::vector<void*> pointers;
    size_t size() const {
        switch (kind) { case Integer: return integers.size(); case Double: return doubles.size(); case String: return strings.size(); default: return pointers.size(); }
    }
};
struct Properties {
    std::map<std::string, Property> values;
    Param* parameter = nullptr;
};
struct Param {
    std::string name, type, string_value;
    int integer_value = 0;
    double double_value = 0;
    Properties properties;
    Effect* effect = nullptr;
    std::map<double, double> double_keys;
};
struct ParamSet {
    Properties properties;
    std::map<std::string, std::unique_ptr<Param>> params;
    Effect* effect = nullptr;
};
struct Clip {
    Properties properties, image_properties;
    std::vector<unsigned char> pixels;
    Effect* effect = nullptr;
    std::string name;
};
struct Effect {
    Properties properties;
    ParamSet parameters;
    std::map<std::string, std::unique_ptr<Clip>> clips;
    bool created = false;
    int number = 0;
    double time = 0;
    int change_depth = 0;
    std::set<std::string> dirty_values;
    std::map<std::string, std::set<std::string>> dirty_properties;
    std::vector<std::string> messages;
    std::string identity_name;
    double identity_time = 0;
    int identity_status = -1;
};

static HMODULE core_module;
static OfxPlugin* plugin;
static OfxHost host;
static Properties host_properties;
static Effect descriptor;
static std::vector<std::unique_ptr<Effect>> instances;
static std::vector<std::string> action_stack;
static std::ofstream trace;
static std::string last_error, snapshot_result;
static std::string messages_result;
static bool strict_value_actions, reentrant_writes;
static bool command_notifications;
static int forbidden_writes, action_number;

static std::string json_quote(const std::string& value) {
    std::ostringstream out;
    out << '"';
    for (unsigned char c : value) {
        switch (c) {
            case '\\': out << "\\\\"; break; case '"': out << "\\\""; break;
            case '\n': out << "\\n"; break; case '\r': out << "\\r"; break; case '\t': out << "\\t"; break;
            default: if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << static_cast<unsigned>(c) << std::dec; else out << c;
        }
    }
    out << '"';
    return out.str();
}
static std::string current_action() { return action_stack.empty() ? "outside_action" : action_stack.back(); }
static void record(const std::string& event, const std::string& fields = "") {
    trace << "{\"event\":" << json_quote(event) << ",\"action\":" << json_quote(current_action()) << ",\"depth\":" << action_stack.size();
    if (!fields.empty()) trace << ',' << fields;
    trace << "}\n";
    trace.flush();
}
static Properties* props(OfxPropertySetHandle p) { return reinterpret_cast<Properties*>(p); }
static Param* param(OfxParamHandle p) { return reinterpret_cast<Param*>(p); }
static ParamSet* paramset(OfxParamSetHandle p) { return reinterpret_cast<ParamSet*>(p); }
static Effect* effect(OfxImageEffectHandle p) { return reinterpret_cast<Effect*>(p); }
static Clip* clip(OfxImageClipHandle p) { return reinterpret_cast<Clip*>(p); }
static OfxPropertySetHandle handle(Properties& p) { return reinterpret_cast<OfxPropertySetHandle>(&p); }

static Property& writable(Properties& p, const char* name, Property::Kind kind) {
    auto& value = p.values[name];
    if (value.kind != kind) value = Property{};
    value.kind = kind;
    return value;
}
static void dirty_property(Properties& p, const char* name) {
    if (p.parameter && p.parameter->effect && p.parameter->effect->created && p.parameter->effect->change_depth > 0)
        p.parameter->effect->dirty_properties[p.parameter->name].insert(name);
}
static int set_int(OfxPropertySetHandle h, const char* name, int index, int value) {
    if (!h || !name || index < 0) return kOfxStatErrBadHandle;
    auto& p = *props(h); auto& values = writable(p, name, Property::Integer).integers;
    if (values.size() <= static_cast<size_t>(index)) values.resize(index + 1);
    int old = values[index]; values[index] = value;
    dirty_property(p,name);
    if (p.parameter && (std::string(name) == kOfxParamPropEnabled || std::string(name) == kOfxParamPropSecret))
        record("property_write", "\"instance\":" + std::to_string(p.parameter->effect ? p.parameter->effect->number : 0) + ",\"parameter\":" + json_quote(p.parameter->name) + ",\"property\":" + json_quote(name) + ",\"before\":" + std::to_string(old) + ",\"after\":" + std::to_string(value));
    return kOfxStatOK;
}
static int set_double(OfxPropertySetHandle h, const char* name, int index, double value) {
    if (!h || !name || index < 0) return kOfxStatErrBadHandle;
    auto& values = writable(*props(h), name, Property::Double).doubles;
    if (values.size() <= static_cast<size_t>(index)) values.resize(index + 1);
    values[index] = value; dirty_property(*props(h),name); return kOfxStatOK;
}
static int set_string(OfxPropertySetHandle h, const char* name, int index, const char* value) {
    if (!h || !name || !value || index < 0) return kOfxStatErrBadHandle;
    auto& p = *props(h); auto& values = writable(p, name, Property::String).strings;
    if (values.size() <= static_cast<size_t>(index)) values.resize(index + 1);
    values[index] = value;
    dirty_property(p,name);
    if (p.parameter && std::string(name) == kOfxPropLabel)
        record("property_write", "\"instance\":" + std::to_string(p.parameter->effect ? p.parameter->effect->number : 0) + ",\"parameter\":" + json_quote(p.parameter->name) + ",\"property\":" + json_quote(name) + ",\"after\":" + json_quote(value));
    return kOfxStatOK;
}
static int set_pointer(OfxPropertySetHandle h, const char* name, int index, void* value) {
    if (!h || !name || index < 0) return kOfxStatErrBadHandle;
    auto& values = writable(*props(h), name, Property::Pointer).pointers;
    if (values.size() <= static_cast<size_t>(index)) values.resize(index + 1);
    values[index] = value; return kOfxStatOK;
}
template<class T, class Pick> static int get_property(OfxPropertySetHandle h, const char* name, int index, T* out, Property::Kind kind, Pick pick) {
    if (!h || !name || !out || index < 0) return kOfxStatErrBadHandle;
    auto found = props(h)->values.find(name);
    if (found == props(h)->values.end()) return kOfxStatErrUnknown;
    if (found->second.kind != kind) return kOfxStatErrValue;
    auto& values = pick(found->second);
    if (static_cast<size_t>(index) >= values.size()) return kOfxStatErrBadIndex;
    *out = values[index]; return kOfxStatOK;
}
static int get_int(OfxPropertySetHandle h, const char* name, int index, int* out) { return get_property(h,name,index,out,Property::Integer,[](Property& p)->auto& { return p.integers; }); }
static int get_double(OfxPropertySetHandle h, const char* name, int index, double* out) { return get_property(h,name,index,out,Property::Double,[](Property& p)->auto& { return p.doubles; }); }
static int get_pointer(OfxPropertySetHandle h, const char* name, int index, void** out) { return get_property(h,name,index,out,Property::Pointer,[](Property& p)->auto& { return p.pointers; }); }
static int get_string(OfxPropertySetHandle h, const char* name, int index, const char** out) {
    if (!h || !name || !out || index < 0) return kOfxStatErrBadHandle;
    auto found = props(h)->values.find(name);
    if (found == props(h)->values.end()) return kOfxStatErrUnknown;
    if (found->second.kind != Property::String) return kOfxStatErrValue;
    if (static_cast<size_t>(index) >= found->second.strings.size()) return kOfxStatErrBadIndex;
    *out = found->second.strings[index].c_str(); return kOfxStatOK;
}
static int set_int_n(OfxPropertySetHandle h,const char* n,int count,const int* v) { for(int i=0;i<count;++i) { int s=set_int(h,n,i,v[i]);if(s)return s; }return 0; }
static int set_double_n(OfxPropertySetHandle h,const char* n,int count,const double* v) { for(int i=0;i<count;++i) { int s=set_double(h,n,i,v[i]);if(s)return s; }return 0; }
static int set_string_n(OfxPropertySetHandle h,const char* n,int count,const char* const* v) { for(int i=0;i<count;++i) { int s=set_string(h,n,i,v[i]);if(s)return s; }return 0; }
static int set_pointer_n(OfxPropertySetHandle h,const char* n,int count,void* const* v) { for(int i=0;i<count;++i) { int s=set_pointer(h,n,i,v[i]);if(s)return s; }return 0; }
static int get_int_n(OfxPropertySetHandle h,const char* n,int count,int* v) { for(int i=0;i<count;++i) { int s=get_int(h,n,i,v+i);if(s)return s; }return 0; }
static int get_double_n(OfxPropertySetHandle h,const char* n,int count,double* v) { for(int i=0;i<count;++i) { int s=get_double(h,n,i,v+i);if(s)return s; }return 0; }
static int get_string_n(OfxPropertySetHandle h,const char* n,int count,const char** v) { for(int i=0;i<count;++i) { int s=get_string(h,n,i,v+i);if(s)return s; }return 0; }
static int get_pointer_n(OfxPropertySetHandle h,const char* n,int count,void** v) { for(int i=0;i<count;++i) { int s=get_pointer(h,n,i,v+i);if(s)return s; }return 0; }
static int reset_property(OfxPropertySetHandle h,const char* n) { if(!h)return kOfxStatErrBadHandle;props(h)->values.erase(n);return 0; }
static int property_dimension(OfxPropertySetHandle h,const char* n,int* out) { if(!h||!out)return kOfxStatErrBadHandle;auto f=props(h)->values.find(n);if(f==props(h)->values.end())return kOfxStatErrUnknown;*out=static_cast<int>(f->second.size());return 0; }
static int integer(Properties& p,const char* name,int fallback=0) { int v=fallback;get_int(handle(p),name,0,&v);return v; }
static std::string string(Properties& p,const char* name) { const char* v=nullptr;return get_string(handle(p),name,0,&v)==0 ? v : ""; }

static int call(Effect*, const char*, Properties*, Properties*);
static int notify(Effect* e, const char* name, const char* reason, double time) {
    for (auto blocked : {"Browse", "LoadLens", "LoadCurrent", "OpenGyroflow", "OpenPluginUpdater", "OpenRecentProject"}) {
        if (std::string(name) == blocked) {
            if (command_notifications && (std::string(name) == "OpenGyroflow" || std::string(name) == "OpenPluginUpdater" || std::string(name) == "Browse")) {
                record("interactive_command_dispatched", "\"parameter\":" + json_quote(name));
                break;
            }
            record("interactive_command_not_dispatched", "\"parameter\":" + json_quote(name));
            return kOfxStatReplyDefault;
        }
    }
    Properties args;
    set_string(handle(args),kOfxPropType,0,kOfxTypeParameter);
    set_string(handle(args),kOfxPropName,0,name);
    set_string(handle(args),kOfxPropChangeReason,0,reason);
    set_double(handle(args),kOfxPropTime,0,time);
    const double scale[]={1,1};set_double_n(handle(args),kOfxImageEffectPropRenderScale,2,scale);
    return call(e,kOfxActionInstanceChanged,&args,nullptr);
}
static std::string param_value(const Param& p) {
    if(p.type==kOfxParamTypeString)return json_quote(p.string_value);
    if(p.type==kOfxParamTypeDouble){std::ostringstream out;out<<std::showpoint<<std::setprecision(17)<<p.double_value;return out.str();}
    if(p.type==kOfxParamTypeBoolean)return p.integer_value?"true":"false";
    if(p.type==kOfxParamTypeGroup||p.type==kOfxParamTypePage)return "null";
    return std::to_string(p.integer_value);
}
static std::string native_property_value(const Property& property, bool strip_ofx=true) {
    std::ostringstream out;
    if(property.size()!=1)out<<'[';
    for(size_t i=0;i<property.size();++i){
        if(i)out<<',';
        switch(property.kind){
            case Property::Integer: out<<property.integers[i];break;
            case Property::Double: out<<std::showpoint<<std::setprecision(17)<<property.doubles[i];break;
            case Property::String: {auto value=property.strings[i];if(strip_ofx&&value.rfind("Ofx",0)==0)value=value.substr(3);out<<json_quote(value);break;}
            default: throw std::runtime_error("Unverified native pointer-property serialization requested");
        }
    }
    if(property.size()!=1)out<<']';
    return out.str();
}
static void flush_type10(Effect* e) {
    if(e->dirty_values.empty()&&e->dirty_properties.empty())return;
    std::map<std::string,std::string> values;
    for(const auto& dirty:e->dirty_properties){
        auto p=e->parameters.params.find(dirty.first);if(p==e->parameters.params.end())continue;
        std::ostringstream object;object<<'{';bool first=true;
        static const std::set<std::string> excluded={"OfxParamPropMax","OfxParamPropMin","OfxPropType","OfxParamPropPersistant","OfxParamPropSecret","OfxParamPropPluginMayWrite"};
        for(const auto& key:dirty.second){if(key.rfind("Ofx",0)!=0||excluded.count(key))continue;auto property=p->second->properties.values.find(key);if(property==p->second->properties.values.end())continue;if(!first)object<<',';first=false;object<<json_quote(key.substr(3))<<':'<<native_property_value(property->second);}
        object<<'}';if(!first)values[dirty.first]=object.str();
    }
    for(const auto& name:e->dirty_values){
        auto& p=*e->parameters.params.at(name);
        if(!integer(p.properties,"OfxParamPropPersistant",1)||p.type==kOfxParamTypePushButton||p.type==kOfxParamTypeGroup||p.type==kOfxParamTypePage)continue;
        values[name]=p.type==kOfxParamTypeBoolean?(p.integer_value?"true":"false"):param_value(p);
    }
    std::ostringstream body;body<<"{\"params\":{";bool first=true;
    for(const auto& value:values){if(!first)body<<',';first=false;body<<json_quote(value.first)<<':'<<value.second;}
    body<<"}}";
    e->messages.push_back("{\"type\":10,\"instance\":"+std::to_string(e->number)+",\"body\":"+json_quote(body.str())+"}");
    record("automatic_type10","\"instance\":"+std::to_string(e->number)+",\"body\":"+body.str());
    e->dirty_values.clear();e->dirty_properties.clear();
}
static bool legal_value_action() {
    const auto a=current_action();
    return a==kOfxActionCreateInstance||a==kOfxActionBeginInstanceChanged||a==kOfxActionInstanceChanged||a==kOfxActionEndInstanceChanged||a==kOfxActionSyncPrivateData;
}
static int before_value_write(Param* p, const char* operation) {
    if(!p)return kOfxStatErrBadHandle;
    const bool legal=legal_value_action();
    record("parameter_write", "\"instance\":"+std::to_string(p->effect->number)+",\"parameter\":"+json_quote(p->name)+",\"operation\":"+json_quote(operation)+",\"legal_action\":"+(legal?"true":"false")+",\"before\":"+param_value(*p));
    if(!legal){++forbidden_writes;if(strict_value_actions)return kOfxStatFailed;}
    return 0;
}
static void after_value_write(Param* p) {
    record("parameter_written", "\"instance\":"+std::to_string(p->effect->number)+",\"parameter\":"+json_quote(p->name)+",\"after\":"+param_value(*p));
    if(p->effect->created)p->effect->dirty_values.insert(p->name);
    if(reentrant_writes && p->effect->created) {
        Properties args;set_string(handle(args),kOfxPropChangeReason,0,kOfxChangePluginEdited);
        call(p->effect,kOfxActionBeginInstanceChanged,&args,nullptr);
        notify(p->effect,p->name.c_str(),kOfxChangePluginEdited,p->effect->time);
        if(integer(p->effect->properties,"OfxPropParamSetNeedsSyncing",1)) {
            call(p->effect,kOfxActionSyncPrivateData,nullptr,nullptr);
            set_int(handle(p->effect->properties),"OfxPropParamSetNeedsSyncing",0,0);
        }
        call(p->effect,kOfxActionEndInstanceChanged,&args,nullptr);
    }
}
static int define_param(OfxParamSetHandle h,const char* type,const char* name,OfxPropertySetHandle* out) {
    if(!h||!type||!name||!out)return kOfxStatErrBadHandle;
    auto* ps=paramset(h);if(ps->params.count(name))return kOfxStatErrExists;
    auto p=std::make_unique<Param>();p->name=name;p->type=type;p->effect=ps->effect;p->properties.parameter=p.get();
    auto hp=handle(p->properties);
    set_string(hp,kOfxPropType,0,kOfxTypeParameter);set_string(hp,kOfxPropName,0,name);set_string(hp,kOfxParamPropType,0,type);set_string(hp,kOfxPropLabel,0,name);
    for(auto flag:{kOfxParamPropEnabled,kOfxParamPropAnimates,"OfxParamPropPersistant",kOfxParamPropCanUndo,kOfxParamPropEvaluateOnChange})set_int(hp,flag,0,1);
    set_int(hp,kOfxParamPropSecret,0,0);
    if(p->type==kOfxParamTypeString){set_string(hp,kOfxParamPropDefault,0,"");set_string(hp,kOfxParamPropStringMode,0,kOfxParamStringIsSingleLine);}
    else if(p->type==kOfxParamTypeDouble)set_double(hp,kOfxParamPropDefault,0,0);
    else set_int(hp,kOfxParamPropDefault,0,0);
    *out=hp;ps->params.emplace(name,std::move(p));return 0;
}
static int param_handle(OfxParamSetHandle h,const char* name,OfxParamHandle* out,OfxPropertySetHandle* p) {
    if(!h||!name||!out)return kOfxStatErrBadHandle;auto f=paramset(h)->params.find(name);if(f==paramset(h)->params.end())return kOfxStatErrUnknown;
    *out=reinterpret_cast<OfxParamHandle>(f->second.get());if(p)*p=handle(f->second->properties);return 0;
}
static int paramset_properties(OfxParamSetHandle h,OfxPropertySetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;*out=handle(paramset(h)->effect->properties);return 0;}
static int param_properties(OfxParamHandle h,OfxPropertySetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;*out=handle(param(h)->properties);return 0;}
static int value_get(Param* p,va_list args,double time,bool timed) {
    if(!p)return kOfxStatErrBadHandle;
    if(p->type==kOfxParamTypeString)*va_arg(args,char**)=const_cast<char*>(p->string_value.c_str());
    else if(p->type==kOfxParamTypeDouble){double v=p->double_value;if(timed&&!p->double_keys.empty()){auto k=p->double_keys.upper_bound(time);if(k!=p->double_keys.begin())--k;v=k->second;std::ostringstream fields;fields<<"\"instance\":"<<p->effect->number<<",\"parameter\":"<<json_quote(p->name)<<",\"time\":"<<std::setprecision(17)<<time;record("keyed_parameter_read_at_time",fields.str());}*va_arg(args,double*)=v;}
    else if(p->type==kOfxParamTypeBoolean||p->type==kOfxParamTypeChoice||p->type==kOfxParamTypeInteger||p->type==kOfxParamTypePushButton) {
        *va_arg(args,int*)=p->integer_value;
        if(p->type==kOfxParamTypePushButton)record("button_counter_read","\"instance\":"+std::to_string(p->effect->number)+",\"parameter\":"+json_quote(p->name)+",\"value\":"+std::to_string(p->integer_value));
    }
    else return kOfxStatErrUnsupported;
    return 0;
}
static int param_get(OfxParamHandle h,...){va_list args;va_start(args,h);int s=value_get(param(h),args,0,false);va_end(args);return s;}
static int param_get_time(OfxParamHandle h,OfxTime time,...){va_list args;va_start(args,time);int s=value_get(param(h),args,time,true);va_end(args);return s;}
static int value_set(Param* p,va_list args,double time,bool timed) {
    int s=before_value_write(p,timed?"paramSetValueAtTime":"paramSetValue");if(s)return s;
    if(p->type==kOfxParamTypeString)p->string_value=va_arg(args,const char*);
    else if(p->type==kOfxParamTypeDouble){p->double_value=va_arg(args,double);if(timed){p->double_keys[time]=p->double_value;std::ostringstream fields;fields<<"\"instance\":"<<p->effect->number<<",\"parameter\":"<<json_quote(p->name)<<",\"time\":"<<std::setprecision(17)<<time<<",\"value\":"<<p->double_value;record("parameter_key_written",fields.str());}}
    else if(p->type==kOfxParamTypeBoolean||p->type==kOfxParamTypeChoice||p->type==kOfxParamTypeInteger||p->type==kOfxParamTypePushButton)p->integer_value=va_arg(args,int);
    else return kOfxStatErrUnsupported;
    after_value_write(p);return 0;
}
static int param_set(OfxParamHandle h,...){va_list args;va_start(args,h);int s=value_set(param(h),args,0,false);va_end(args);return s;}
static int param_set_time(OfxParamHandle h,OfxTime time,...){va_list args;va_start(args,time);int s=value_set(param(h),args,time,true);va_end(args);return s;}
static int num_keys(OfxParamHandle h,unsigned int* out){if(!h||!out)return kOfxStatErrBadHandle;*out=static_cast<unsigned>(param(h)->double_keys.size());return 0;}
static int key_time(OfxParamHandle h,unsigned int index,OfxTime* out){if(!h||!out||index>=param(h)->double_keys.size())return kOfxStatErrBadIndex;auto i=param(h)->double_keys.begin();std::advance(i,index);*out=i->first;return 0;}
static int delete_keys(OfxParamHandle h){int s=before_value_write(param(h),"paramDeleteAllKeys");if(s)return s;param(h)->double_keys.clear();return 0;}
static int edit_begin(OfxParamSetHandle,const char*){return 0;}static int edit_end(OfxParamSetHandle){return 0;}

static int effect_properties(OfxImageEffectHandle h,OfxPropertySetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;*out=handle(effect(h)->properties);return 0;}
static int effect_paramset(OfxImageEffectHandle h,OfxParamSetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;*out=reinterpret_cast<OfxParamSetHandle>(&effect(h)->parameters);return 0;}
static int define_clip(OfxImageEffectHandle h,const char* name,OfxPropertySetHandle* out) {
    if(!h||!name||!out)return kOfxStatErrBadHandle;auto* e=effect(h);auto c=std::make_unique<Clip>();c->name=name;c->effect=e;
    set_string(handle(c->properties),kOfxPropName,0,name);set_string(handle(c->properties),kOfxPropType,0,kOfxTypeClip);*out=handle(c->properties);e->clips.emplace(name,std::move(c));return 0;
}
static int clip_handle(OfxImageEffectHandle h,const char* name,OfxImageClipHandle* out,OfxPropertySetHandle* p){if(!h||!name||!out)return kOfxStatErrBadHandle;auto f=effect(h)->clips.find(name);if(f==effect(h)->clips.end())return kOfxStatErrUnknown;*out=reinterpret_cast<OfxImageClipHandle>(f->second.get());if(p)*p=handle(f->second->properties);return 0;}
static int clip_properties(OfxImageClipHandle h,OfxPropertySetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;*out=handle(clip(h)->properties);return 0;}
static int clip_rod(OfxImageClipHandle,OfxTime,OfxRectD* out){if(!out)return kOfxStatErrBadHandle;*out={0,0,3840,2160};return 0;}
static int clip_image(OfxImageClipHandle h,OfxTime time,const OfxRectD*,OfxPropertySetHandle* out){if(!h||!out)return kOfxStatErrBadHandle;auto* c=clip(h);*out=handle(c->image_properties);record("clip_image","\"instance\":"+std::to_string(c->effect->number)+",\"clip\":"+json_quote(c->name)+",\"time\":"+std::to_string(time));return 0;}
static int release_image(OfxPropertySetHandle){return 0;}static int abort_render(OfxImageEffectHandle){return 0;}
static int memory_alloc(void*,size_t bytes,void** out){*out=std::malloc(bytes);return *out?0:kOfxStatErrMemory;}static int memory_free(void* p){std::free(p);return 0;}
static int image_alloc(OfxImageEffectHandle,size_t bytes,OfxImageMemoryHandle* out){void* p=std::malloc(bytes);*out=reinterpret_cast<OfxImageMemoryHandle>(p);return p?0:kOfxStatErrMemory;}static int image_free(OfxImageMemoryHandle h){std::free(h);return 0;}static int image_lock(OfxImageMemoryHandle h,void** out){*out=h;return 0;}static int image_unlock(OfxImageMemoryHandle){return 0;}
static int multi_thread(OfxThreadFunctionV1* fn,unsigned n,void* args){if(!n)n=1;for(unsigned i=0;i<n;++i)fn(i,n,args);return 0;}static int num_cpus(unsigned* out){*out=1;return 0;}static int thread_index(unsigned* out){*out=0;return 0;}static int spawned(){return 0;}
static int mutex_create(OfxMutexHandle* out,int count){auto* m=new std::recursive_mutex;for(int i=0;i<count;++i)m->lock();*out=reinterpret_cast<OfxMutexHandle>(m);return 0;}static int mutex_destroy(OfxMutexHandle h){delete reinterpret_cast<std::recursive_mutex*>(h);return 0;}static int mutex_lock(OfxMutexHandle h){reinterpret_cast<std::recursive_mutex*>(h)->lock();return 0;}static int mutex_unlock(OfxMutexHandle h){reinterpret_cast<std::recursive_mutex*>(h)->unlock();return 0;}static int mutex_try(OfxMutexHandle h){return reinterpret_cast<std::recursive_mutex*>(h)->try_lock()?0:kOfxStatFailed;}
static int message(void*,const char* type,const char* id,const char*,...){record("message","\"type\":"+json_quote(type?type:"")+",\"id\":"+json_quote(id?id:""));return kOfxStatReplyDefault;}
static int progress_start(void*,const char*){return 0;}static int progress_update(void*,double){return 0;}static int progress_end(void*){return 0;}
static int time_get(void* h,OfxTime* out){*out=reinterpret_cast<Effect*>(h)->time;return 0;}static int time_goto(void*,OfxTime){return kOfxStatErrUnsupported;}static int time_bounds(void*,OfxTime* a,OfxTime* b){*a=0;*b=300.99999;return 0;}

static OfxPropertySuiteV1 property_suite{};
static OfxParameterSuiteV1 parameter_suite{};
static OfxImageEffectSuiteV1 image_suite{};
static OfxMemorySuiteV1 memory_suite{};
static OfxMultiThreadSuiteV1 thread_suite{};
static OfxMessageSuiteV1 message_suite{};
static OfxProgressSuiteV1 progress_suite{};
static OfxTimeLineSuiteV1 timeline_suite{};
static const void* fetch_suite(OfxPropertySetHandle,const char* name,int version) {
    if(version!=1)return nullptr;
    if(std::string(name)==kOfxPropertySuite)return &property_suite;
    if(std::string(name)==kOfxParameterSuite)return &parameter_suite;
    if(std::string(name)==kOfxImageEffectSuite)return &image_suite;
    if(std::string(name)==kOfxMemorySuite)return &memory_suite;
    if(std::string(name)==kOfxMultiThreadSuite)return &thread_suite;
    if(std::string(name)==kOfxMessageSuite)return &message_suite;
    if(std::string(name)==kOfxProgressSuite)return &progress_suite;
    if(std::string(name)==kOfxTimeLineSuite)return &timeline_suite;
    return nullptr;
}
static void init_suites() {
    property_suite={set_pointer,set_string,set_double,set_int,set_pointer_n,set_string_n,set_double_n,set_int_n,get_pointer,get_string,get_double,get_int,get_pointer_n,get_string_n,get_double_n,get_int_n,reset_property,property_dimension};
    parameter_suite.paramDefine=define_param;parameter_suite.paramGetHandle=param_handle;parameter_suite.paramSetGetPropertySet=paramset_properties;parameter_suite.paramGetPropertySet=param_properties;parameter_suite.paramGetValue=param_get;parameter_suite.paramGetValueAtTime=param_get_time;parameter_suite.paramSetValue=param_set;parameter_suite.paramSetValueAtTime=param_set_time;parameter_suite.paramGetNumKeys=num_keys;parameter_suite.paramGetKeyTime=key_time;parameter_suite.paramDeleteAllKeys=delete_keys;parameter_suite.paramEditBegin=edit_begin;parameter_suite.paramEditEnd=edit_end;
    image_suite={effect_properties,effect_paramset,define_clip,clip_handle,clip_properties,clip_image,release_image,clip_rod,abort_render,image_alloc,image_free,image_lock,image_unlock};
    memory_suite={memory_alloc,memory_free};thread_suite={multi_thread,num_cpus,thread_index,spawned,mutex_create,mutex_destroy,mutex_lock,mutex_unlock,mutex_try};message_suite={message};progress_suite={progress_start,progress_update,progress_end};timeline_suite={time_get,time_goto,time_bounds};
}
static int call(Effect* e,const char* action,Properties* in,Properties* out) {
    if(e&&std::string(action)==kOfxActionBeginInstanceChanged)++e->change_depth;
    action_stack.emplace_back(action);
    const int id=++action_number;
    std::string fields="\"id\":"+std::to_string(id)+",\"instance\":"+std::to_string(e?e->number:0);
    if(in){std::string name=string(*in,kOfxPropName),reason=string(*in,kOfxPropChangeReason);if(!name.empty())fields+=",\"parameter\":"+json_quote(name);if(!reason.empty())fields+=",\"reason\":"+json_quote(reason);}
    record("action_begin",fields);
    int status=plugin->mainEntry(action,e,in?handle(*in):nullptr,out?handle(*out):nullptr);
    if(e&&std::string(action)==kOfxActionEndInstanceChanged){if(e->change_depth>0)--e->change_depth;if(e->change_depth==0)flush_type10(e);}
    record("action_end","\"id\":"+std::to_string(id)+",\"instance\":"+std::to_string(e?e->number:0)+",\"status\":"+std::to_string(status));
    action_stack.pop_back();return status;
}

extern "C" __declspec(dllexport) int fixture_open(const wchar_t* core_path,const wchar_t* trace_path,int strict,int reentrant) {
    try {
        trace.open(trace_path,std::ios::binary);if(!trace)throw std::runtime_error("Cannot open fixture trace");
        strict_value_actions=strict!=0;reentrant_writes=reentrant!=0;command_notifications=false;init_suites();
        set_string(handle(host_properties),kOfxPropName,0,"com.lemon.lvpro");set_string(handle(host_properties),kOfxPropVersionLabel,0,"11.6");set_int(handle(host_properties),kOfxParamHostPropSupportsCustomInteract,0,0);
        for(auto name:{"OfxImageEffectPropOpenGLRenderSupported","OfxImageEffectPropOpenCLRenderSupported","OfxImageEffectPropCudaRenderSupported","OfxImageEffectPropMetalRenderSupported","OfxImageEffectPropAngleRenderSupported"})set_string(handle(host_properties),name,0,"false");
        host={handle(host_properties),fetch_suite};core_module=LoadLibraryW(core_path);if(!core_module)throw std::runtime_error("Cannot load core DLL: "+std::to_string(GetLastError()));
        auto get=reinterpret_cast<OfxPlugin*(*)(int)>(GetProcAddress(core_module,"OfxGetPlugin"));if(!get)throw std::runtime_error("Missing OfxGetPlugin");plugin=get(0);plugin->setHost(&host);
        if(call(nullptr,kOfxActionLoad,nullptr,nullptr)!=0)throw std::runtime_error("Load failed");
        descriptor.parameters.effect=&descriptor;set_string(handle(descriptor.properties),kOfxPropType,0,kOfxTypeImageEffect);set_string(handle(descriptor.properties),kOfxImageEffectPropContext,0,kOfxImageEffectContextFilter);
        if(call(&descriptor,kOfxActionDescribe,nullptr,nullptr)!=0)throw std::runtime_error("Describe failed");
        Properties args;set_string(handle(args),kOfxImageEffectPropContext,0,kOfxImageEffectContextFilter);
        if(call(&descriptor,kOfxImageEffectActionDescribeInContext,&args,nullptr)!=0)throw std::runtime_error("DescribeInContext failed");
        return 0;
    }catch(const std::exception& e){last_error=e.what();return -1;}
}
extern "C" __declspec(dllexport) void* fixture_new_instance() {
    auto e=std::make_unique<Effect>();e->number=static_cast<int>(instances.size()+1);e->parameters.effect=e.get();e->properties=descriptor.properties;
    set_string(handle(e->properties),kOfxPropType,0,kOfxTypeImageEffectInstance);set_double(handle(e->properties),kOfxImageEffectPropFrameRate,0,30);
    const double project_size[]={3840,2160};set_double_n(handle(e->properties),kOfxImageEffectPropProjectSize,2,project_size);set_pointer(handle(e->properties),kOfxPropInstanceData,0,nullptr);
    for(auto& entry:descriptor.parameters.params){auto p=std::make_unique<Param>();p->name=entry.first;p->type=entry.second->type;p->properties=entry.second->properties;p->effect=e.get();p->properties.parameter=p.get();set_string(handle(p->properties),kOfxPropType,0,kOfxTypeParameterInstance);if(p->type==kOfxParamTypeString)p->string_value=string(p->properties,kOfxParamPropDefault);else if(p->type==kOfxParamTypeDouble)get_double(handle(p->properties),kOfxParamPropDefault,0,&p->double_value);else p->integer_value=integer(p->properties,kOfxParamPropDefault);e->parameters.params.emplace(entry.first,std::move(p));}
    for(auto& entry:descriptor.clips){auto c=std::make_unique<Clip>();c->name=entry.first;c->effect=e.get();c->properties=entry.second->properties;auto cp=handle(c->properties);set_double(cp,kOfxImageEffectPropFrameRate,0,30);set_double(cp,kOfxImageEffectPropUnmappedFrameRate,0,30);const double range[]={0,300.99999};set_double_n(cp,kOfxImageEffectPropFrameRange,2,range);set_double_n(cp,kOfxImageEffectPropUnmappedFrameRange,2,range);set_double(cp,kOfxImagePropPixelAspectRatio,0,1);
        c->pixels.resize(480*270*4);for(size_t i=0;i<c->pixels.size();++i)c->pixels[i]=c->name=="Source"?static_cast<unsigned char>((i*17+i/1920)%251):0;
        auto ip=handle(c->image_properties);set_pointer(ip,kOfxImagePropData,0,c->pixels.data());const int rect[]={0,0,480,270};set_int_n(ip,kOfxImagePropBounds,4,rect);set_int_n(ip,kOfxImagePropRegionOfDefinition,4,rect);set_int(ip,kOfxImagePropRowBytes,0,480*4);set_string(ip,kOfxImageEffectPropPixelDepth,0,kOfxBitDepthByte);set_string(ip,kOfxImageEffectPropComponents,0,kOfxImageComponentRGBA);set_string(ip,kOfxImageEffectPropPreMultiplication,0,kOfxImageOpaque);set_double(ip,kOfxImagePropPixelAspectRatio,0,1);const double scale[]={0.125,0.125};set_double_n(ip,kOfxImageEffectPropRenderScale,2,scale);e->clips.emplace(entry.first,std::move(c));}
    auto result=e.get();instances.push_back(std::move(e));return result;
}
extern "C" __declspec(dllexport) int fixture_create(void* ptr){auto* e=static_cast<Effect*>(ptr);int s=call(e,kOfxActionCreateInstance,nullptr,nullptr);e->created=s==0;return s;}
extern "C" __declspec(dllexport) int fixture_import_value(void* ptr,const char* name,int kind,double number,const char* text) {
    auto* e=static_cast<Effect*>(ptr);auto f=e->parameters.params.find(name);if(f==e->parameters.params.end())return kOfxStatErrUnknown;auto* p=f->second.get();
    if(kind==2&&p->type==kOfxParamTypeString)p->string_value=text?text:"";
    else if(kind==1&&p->type==kOfxParamTypeDouble)p->double_value=number;
    else if(kind==0&&(p->type==kOfxParamTypeBoolean||p->type==kOfxParamTypeChoice||p->type==kOfxParamTypeInteger||p->type==kOfxParamTypePushButton))p->integer_value=static_cast<int>(number);
    else return kOfxStatErrValue;
    record("host_import","\"instance\":"+std::to_string(e->number)+",\"parameter\":"+json_quote(name)+",\"value\":"+param_value(*p));return 0;
}
extern "C" __declspec(dllexport) int fixture_import_keyframe(void* ptr,const char* name,double time,double value){auto* e=static_cast<Effect*>(ptr);auto p=e->parameters.params.find(name);if(p==e->parameters.params.end()||p->second->type!=kOfxParamTypeDouble)return kOfxStatErrValue;p->second->double_keys[time]=value;return 0;}
extern "C" __declspec(dllexport) void fixture_allow_command_notifications(int enabled){command_notifications=enabled!=0;}
extern "C" __declspec(dllexport) int fixture_set_source_path(void* ptr,const char* path) {
    if(!ptr)return kOfxStatErrBadHandle;
    auto* e=static_cast<Effect*>(ptr);
    if(path)set_string(handle(e->properties),"NiYienJianyingSourcePath",0,path);
    else e->properties.values.erase("NiYienJianyingSourcePath");
    record("host_source_path","\"instance\":"+std::to_string(e->number)+",\"value\":"+(path?json_quote(path):"null"));
    return kOfxStatOK;
}
extern "C" __declspec(dllexport) int fixture_notify(void* ptr,const char* name,const char* reason,double time){auto* e=static_cast<Effect*>(ptr);e->time=time;return notify(e,name,reason,time);}
extern "C" __declspec(dllexport) int fixture_action(void* ptr,const char* action,const char* reason,double time) {
    auto* e=static_cast<Effect*>(ptr);e->time=time;if(std::string(action)==kOfxImageEffectActionRender){auto& pixels=e->clips.at("Output")->pixels;std::fill(pixels.begin(),pixels.end(),0);}Properties in,out;set_string(handle(in),kOfxPropChangeReason,0,reason?reason:kOfxChangeUserEdited);set_double(handle(in),kOfxPropTime,0,time);const double scale[]={0.125,0.125};set_double_n(handle(in),kOfxImageEffectPropRenderScale,2,scale);const int rect[]={0,0,480,270};set_int_n(handle(in),kOfxImageEffectPropRenderWindow,4,rect);for(auto n:{"OfxImageEffectPropOpenGLEnabled","OfxImageEffectPropOpenCLEnabled","OfxImageEffectPropCudaEnabled","OfxImageEffectPropMetalEnabled"})set_int(handle(in),n,0,0);
    const int status=call(e,action,&in,&out);
    if(std::string(action)==kOfxImageEffectActionIsIdentity) {
        e->identity_name=string(out,kOfxPropName);e->identity_time=0;get_double(handle(out),kOfxPropTime,0,&e->identity_time);e->identity_status=status;
        std::ostringstream fields;fields<<"\"instance\":"<<e->number<<",\"status\":"<<status<<",\"name\":"<<json_quote(e->identity_name)<<",\"time\":"<<std::setprecision(17)<<e->identity_time;record("identity_result",fields.str());
    }
    return status;
}
extern "C" __declspec(dllexport) const unsigned char* fixture_output_data(void* ptr,size_t* bytes){auto& pixels=static_cast<Effect*>(ptr)->clips.at("Output")->pixels;*bytes=pixels.size();return pixels.data();}
extern "C" __declspec(dllexport) const unsigned char* fixture_source_data(void* ptr,size_t* bytes){auto& pixels=static_cast<Effect*>(ptr)->clips.at("Source")->pixels;*bytes=pixels.size();return pixels.data();}
extern "C" __declspec(dllexport) const char* fixture_snapshot(void* ptr) {
    auto* e=static_cast<Effect*>(ptr);std::ostringstream out;out<<"{\"instance\":"<<e->number<<",\"parameters\":{";bool first=true;
    for(auto& entry:e->parameters.params){auto& p=*entry.second;if(!first)out<<',';first=false;out<<json_quote(p.name)<<":{\"type\":"<<json_quote(p.type)<<",\"value\":"<<param_value(p)<<",\"enabled\":"<<(integer(p.properties,kOfxParamPropEnabled,1)?"true":"false")<<",\"secret\":"<<(integer(p.properties,kOfxParamPropSecret)?"true":"false")<<",\"label\":"<<json_quote(string(p.properties,kOfxPropLabel))<<",\"string_mode\":"<<json_quote(string(p.properties,kOfxParamPropStringMode))<<",\"parent_name\":"<<json_quote(string(p.properties,kOfxParamPropParent))<<",\"persistant\":"<<(integer(p.properties,"OfxParamPropPersistant",1)?"true":"false")<<",\"animates\":"<<(integer(p.properties,kOfxParamPropAnimates,1)?"true":"false")<<",\"evaluate_on_change\":"<<(integer(p.properties,kOfxParamPropEvaluateOnChange,1)?"true":"false")<<",\"can_undo\":"<<(integer(p.properties,kOfxParamPropCanUndo,1)?"true":"false");auto d=p.properties.values.find(kOfxParamPropDefault);out<<",\"default_value\":"<<(d==p.properties.values.end()?"null":native_property_value(d->second,false));auto children=p.properties.values.find(kOfxParamPropPageChild);out<<",\"page_children\":"<<(children==p.properties.values.end()?"[]":native_property_value(children->second,false))<<'}';}
    out<<"},\"forbidden_writes\":"<<forbidden_writes<<",\"last_identity\":{\"status\":"<<e->identity_status<<",\"name\":"<<json_quote(e->identity_name)<<",\"time\":"<<std::setprecision(17)<<e->identity_time<<"}}";snapshot_result=out.str();return snapshot_result.c_str();
}
extern "C" __declspec(dllexport) const char* fixture_error(){return last_error.c_str();}
extern "C" __declspec(dllexport) const char* fixture_take_messages(void* ptr){auto* e=static_cast<Effect*>(ptr);std::ostringstream out;out<<'[';bool first=true;for(const auto& message:e->messages){if(!first)out<<',';first=false;out<<message;}out<<']';e->messages.clear();messages_result=out.str();return messages_result.c_str();}
extern "C" __declspec(dllexport) void fixture_close(){for(auto& e:instances)if(e->created){call(e.get(),kOfxActionDestroyInstance,nullptr,nullptr);e->created=false;}call(nullptr,kOfxActionUnload,nullptr,nullptr);trace.flush();}
