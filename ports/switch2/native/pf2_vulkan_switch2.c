// SPDX-License-Identifier: MIT OR Apache-2.0
// Switch 2 OunceVK Vulkan scan-out of caller-provided RGBA pixels.
// This owns Vulkan swapchain/presenter resources; Rust wraps the borrowed GPU
// render target with Skia Ganesh. The presenter never owns Ganesh objects.
#include "pf2_native.h"
#if !defined(__SWITCH2__)
#error "OunceVK target required"
#endif
#define VK_USE_PLATFORM_VI_NN 1
#include <switch.h>
#include <vulkan/vulkan.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <stdio.h>

#define PF2_MAX_SWAP 8
struct Pf2Presenter {
    VkInstance instance;
    VkSurfaceKHR surface;
    VkPhysicalDevice physical;
    VkDevice device;
    VkQueue queue;
    uint32_t queue_family;
    VkSwapchainKHR swap;
    VkImage images[PF2_MAX_SWAP];
    uint32_t image_count;
    VkExtent2D extent;
    VkFormat format;
    // External offscreen render target borrowed by Skia; never owned by Skia.
    VkImage gpu_image;
    VkDeviceMemory gpu_memory;
    VkFormat gpu_format;
    VkBuffer upload;
    VkDeviceMemory upload_memory;
    void *mapped;
    size_t upload_bytes;
    bool host_coherent;
    bool invalidated;  // A changed extent requires close/open to rebuild the upload buffer.
    bool device_lost; // Never try to repair an irrecoverable VkDevice on the render thread.
    VkCommandPool pool;
    VkCommandBuffer cmd;
    VkSemaphore acquire;
    VkSemaphore complete;
    VkFence fence;
    uint32_t desired_w, desired_h;
    uint32_t refresh_policy_rc;
    uint32_t size_policy_rc;
    uint32_t tv_width, tv_height, tv_fps;
};

static uint32_t find_memory(VkPhysicalDevice phy, uint32_t bits,
                             VkMemoryPropertyFlags wanted) {
    VkPhysicalDeviceMemoryProperties props;
    vkGetPhysicalDeviceMemoryProperties(phy, &props);
    for (uint32_t i=0; i<props.memoryTypeCount; i++)
        if ((bits & (1u << i)) &&
            (props.memoryTypes[i].propertyFlags & wanted) == wanted) return i;
    return UINT32_MAX;
}
static uint32_t clamp_u32(uint32_t x,uint32_t lo,uint32_t hi) {
    if (x < lo) return lo;
    if (x > hi) return hi;
    return x;
}

// Probe the *current* physical surface, not the console's generic HDR
// marketing capability. Native HDR10 output is not enabled by this probe:
// the existing Ganesh render target and swapchain are still RGBA8 SDR.
Pf2Error pf2_presenter_hdr10_surface(const Pf2Presenter *p, uint32_t *supported) {
    if (!p || !supported || !p->surface || !p->physical) return PF2_INVALID;
    *supported = 0;
    uint32_t count = 0;
    VkResult rc = vkGetPhysicalDeviceSurfaceFormatsKHR(p->physical, p->surface, &count, NULL);
    if (rc != VK_SUCCESS || count == 0 || count > 1024) return PF2_SDK_ERROR;
    VkSurfaceFormatKHR *formats = calloc(count, sizeof(*formats));
    if (!formats) return PF2_NO_MEMORY;
    rc = vkGetPhysicalDeviceSurfaceFormatsKHR(p->physical, p->surface, &count, formats);
    if (rc == VK_SUCCESS) {
        for (uint32_t i=0; i<count; ++i) {
            if (formats[i].colorSpace == VK_COLOR_SPACE_HDR10_ST2084_EXT &&
                (formats[i].format == VK_FORMAT_A2B10G10R10_UNORM_PACK32 ||
                 formats[i].format == VK_FORMAT_A2R10G10B10_UNORM_PACK32)) {
                *supported = 1;
                break;
            }
        }
    }
    free(formats);
    return rc == VK_SUCCESS ? PF2_OK : PF2_SDK_ERROR;
}

static Pf2Error create_swap(Pf2Presenter *p) {
    VkSurfaceCapabilitiesKHR caps;
    if (vkGetPhysicalDeviceSurfaceCapabilitiesKHR(p->physical,p->surface,&caps)!=VK_SUCCESS)
        return PF2_SDK_ERROR;
    if (!(caps.supportedUsageFlags & VK_IMAGE_USAGE_TRANSFER_DST_BIT)) return PF2_UNSUPPORTED;
    uint32_t count=0;
    if (vkGetPhysicalDeviceSurfaceFormatsKHR(p->physical,p->surface,&count,NULL)!=VK_SUCCESS || !count)
        return PF2_SDK_ERROR;
    VkSurfaceFormatKHR *formats=calloc(count,sizeof(*formats));
    if (!formats) return PF2_NO_MEMORY;
    VkResult rc=vkGetPhysicalDeviceSurfaceFormatsKHR(p->physical,p->surface,&count,formats);
    if (rc!=VK_SUCCESS) { free(formats); return PF2_SDK_ERROR; }
    VkSurfaceFormatKHR selected={0};
    bool found=false;
    for (uint32_t i=0;i<count;i++) {
        if (formats[i].format==VK_FORMAT_R8G8B8A8_UNORM ||
            formats[i].format==VK_FORMAT_B8G8R8A8_UNORM) {
            selected=formats[i];found=true;
            if (selected.format==VK_FORMAT_R8G8B8A8_UNORM) break;
        }
    }
    free(formats);
    if (!found) return PF2_UNSUPPORTED;
    uint32_t n=caps.minImageCount+1;
    if (caps.maxImageCount && n>caps.maxImageCount) n=caps.maxImageCount;
    if (n>PF2_MAX_SWAP) return PF2_UNSUPPORTED;
    VkExtent2D extent=caps.currentExtent;
    if (extent.width==UINT32_MAX) {
        extent.width=clamp_u32(p->desired_w,caps.minImageExtent.width,caps.maxImageExtent.width);
        extent.height=clamp_u32(p->desired_h,caps.minImageExtent.height,caps.maxImageExtent.height);
    }
    // Zero extent is legal while a window is minimized / changing display mode.
    // Never create a swapchain or divide by height before the surface settles.
    if (extent.width < 2 || extent.height < 2 ||
        extent.width > 3840 || extent.height > 2160) return PF2_OUT_OF_DATE;
    VkCompositeAlphaFlagBitsKHR alpha=VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
    if (!(caps.supportedCompositeAlpha & alpha)) {
        if (caps.supportedCompositeAlpha & VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR)
            alpha=VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR;
        else return PF2_UNSUPPORTED;
    }
    VkSurfaceTransformFlagBitsKHR transform = (caps.supportedTransforms & VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR)
        ? VK_SURFACE_TRANSFORM_IDENTITY_BIT_KHR : caps.currentTransform;
    VkSwapchainCreateInfoKHR info={0};
    info.sType=VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR;
    info.surface=p->surface;
    info.minImageCount=n;
    info.imageFormat=selected.format;
    info.imageColorSpace=selected.colorSpace;
    info.imageExtent=extent;
    info.imageArrayLayers=1;
    info.imageUsage=VK_IMAGE_USAGE_TRANSFER_DST_BIT;
    info.imageSharingMode=VK_SHARING_MODE_EXCLUSIVE;
    info.preTransform=transform;
    info.compositeAlpha=alpha;
    info.presentMode=VK_PRESENT_MODE_FIFO_KHR;
    info.clipped=VK_TRUE;
    info.oldSwapchain=p->swap;
    VkSwapchainKHR next=VK_NULL_HANDLE;
    rc=vkCreateSwapchainKHR(p->device,&info,NULL,&next);
    if (rc!=VK_SUCCESS) return PF2_SDK_ERROR;
    count=0;
    rc=vkGetSwapchainImagesKHR(p->device,next,&count,NULL);
    if (rc!=VK_SUCCESS || count>PF2_MAX_SWAP || !count) {
        vkDestroySwapchainKHR(p->device,next,NULL);return PF2_UNSUPPORTED;
    }
    VkImage images[PF2_MAX_SWAP]={0};
    rc=vkGetSwapchainImagesKHR(p->device,next,&count,images);
    if (rc!=VK_SUCCESS) {vkDestroySwapchainKHR(p->device,next,NULL);return PF2_SDK_ERROR;}
    if (p->swap) vkDestroySwapchainKHR(p->device,p->swap,NULL);
    p->swap=next;p->extent=extent;p->format=selected.format;p->image_count=count;
    memcpy(p->images,images,count*sizeof(VkImage));
    return PF2_OK;
}

static void close_upload(Pf2Presenter *p) {
    if (p->mapped) vkUnmapMemory(p->device,p->upload_memory);
    if (p->upload) vkDestroyBuffer(p->device,p->upload,NULL);
    if (p->upload_memory) vkFreeMemory(p->device,p->upload_memory,NULL);
    p->mapped=NULL;p->upload=VK_NULL_HANDLE;p->upload_memory=VK_NULL_HANDLE;
    p->upload_bytes=0;
}

static Pf2Error open_upload(Pf2Presenter *p) {
    if (p->upload || p->upload_memory) close_upload(p);
    if (p->extent.width < 2 || p->extent.height < 2) return PF2_OUT_OF_DATE;
    if ((size_t)p->extent.width > SIZE_MAX / (size_t)p->extent.height / 4) return PF2_INVALID;
    size_t bytes=(size_t)p->extent.width*p->extent.height*4;
    VkBufferCreateInfo bi={0};
    bi.sType=VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO;
    bi.size=bytes;bi.usage=VK_BUFFER_USAGE_TRANSFER_SRC_BIT;
    bi.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
    if (vkCreateBuffer(p->device,&bi,NULL,&p->upload)!=VK_SUCCESS) return PF2_SDK_ERROR;
    VkMemoryRequirements req;
    vkGetBufferMemoryRequirements(p->device,p->upload,&req);
    uint32_t type=find_memory(p->physical,req.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT|VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
    p->host_coherent=true;
    if (type==UINT32_MAX) {
        type=find_memory(p->physical,req.memoryTypeBits,VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT);
        p->host_coherent=false;
    }
    if (type==UINT32_MAX) return PF2_UNSUPPORTED;
    VkMemoryAllocateInfo ai={0};
    ai.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
    ai.allocationSize=req.size;
    ai.memoryTypeIndex=type;
    if (vkAllocateMemory(p->device,&ai,NULL,&p->upload_memory)!=VK_SUCCESS) return PF2_NO_MEMORY;
    if (vkBindBufferMemory(p->device,p->upload,p->upload_memory,0)!=VK_SUCCESS) return PF2_SDK_ERROR;
    if (vkMapMemory(p->device,p->upload_memory,0,VK_WHOLE_SIZE,0,&p->mapped)!=VK_SUCCESS)
        return PF2_SDK_ERROR;
    p->upload_bytes=bytes;
    return PF2_OK;
}
// Only call after the device is idle. A failed queue submit leaves a reset
// fence unsignaled; recreating BOTH binary semaphores and the fence also
// discards acquire signals that were never consumed by a successful submit.
static Pf2Error renew_frame_sync(Pf2Presenter *p) {
    if (p->fence) vkDestroyFence(p->device,p->fence,NULL);
    if (p->acquire) vkDestroySemaphore(p->device,p->acquire,NULL);
    if (p->complete) vkDestroySemaphore(p->device,p->complete,NULL);
    p->fence=VK_NULL_HANDLE;
    p->acquire=VK_NULL_HANDLE;
    p->complete=VK_NULL_HANDLE;
    VkSemaphoreCreateInfo si={0};si.sType=VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO;
    VkFenceCreateInfo fi={0};fi.sType=VK_STRUCTURE_TYPE_FENCE_CREATE_INFO;
    fi.flags=VK_FENCE_CREATE_SIGNALED_BIT;
    if (vkCreateSemaphore(p->device,&si,NULL,&p->acquire)!=VK_SUCCESS ||
        vkCreateSemaphore(p->device,&si,NULL,&p->complete)!=VK_SUCCESS ||
        vkCreateFence(p->device,&fi,NULL,&p->fence)!=VK_SUCCESS) {
        // A partially constructed sync set is never exposed as usable.
        if (p->acquire) vkDestroySemaphore(p->device,p->acquire,NULL);
        if (p->complete) vkDestroySemaphore(p->device,p->complete,NULL);
        if (p->fence) vkDestroyFence(p->device,p->fence,NULL);
        p->acquire=VK_NULL_HANDLE;p->complete=VK_NULL_HANDLE;p->fence=VK_NULL_HANDLE;
        return PF2_SDK_ERROR;
    }
    return PF2_OK;
}

// After an acquire succeeded, *any* failure before a completed present
// invalidates the binary semaphore/command-buffer cycle. Ask Rust to drop its
// borrowed Ganesh surface, then resize/rebuild sync on this same VkDevice.
static Pf2Error broken_frame(Pf2Presenter *p, VkResult result) {
    p->invalidated=true;
    if (result == VK_ERROR_DEVICE_LOST) {
        p->device_lost=true;
        pf2_runtime_boot_trace("presenter.vulkan.device_lost");
        return PF2_SDK_ERROR;
    }
    pf2_runtime_boot_trace("presenter.vulkan.frame_failed.recovery_required");
    return PF2_OUT_OF_DATE;
}

static Pf2Error frame_objects(Pf2Presenter *p) {
    VkCommandPoolCreateInfo pci={0};
    pci.sType=VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO;
    pci.flags=VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT;
    pci.queueFamilyIndex=p->queue_family;
    if (vkCreateCommandPool(p->device,&pci,NULL,&p->pool)!=VK_SUCCESS) return PF2_SDK_ERROR;
    VkCommandBufferAllocateInfo cai={0};
    cai.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO;
    cai.commandPool=p->pool;cai.commandBufferCount=1;cai.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY;
    if (vkAllocateCommandBuffers(p->device,&cai,&p->cmd)!=VK_SUCCESS) return PF2_SDK_ERROR;
    return renew_frame_sync(p);
}

// Request, do not assume, 120 Hz. A docked 4K60 stream must not force
// the TV into a lower 120-Hz mode; only a selected <=1440p@120 can ask.
// Moonlight NX2 makes the same distinction and probes the accepted VI2 rate.
static void pf2_apply_window_frame_rates(Pf2Presenter *p) {
    const bool docked = appletGetOperationMode() == AppletOperationMode_Console;
    const bool fast_tv = docked && p->tv_fps == 120 &&
                         p->tv_height > 0 && p->tv_height <= 1440;
    p->refresh_policy_rc=(uint32_t)nwindowSetFrameRates(nwindowGetDefault(),
                                                        (!docked || fast_tv) ? 120 : 60,
                                                        60, !docked || fast_tv);
}

Pf2Error pf2_presenter_request_tv_mode(Pf2Presenter *p, uint32_t width,
                                        uint32_t height, uint32_t fps) {
    if (!p || !p->device) return PF2_INVALID;
    if ((width != 0 || height != 0) &&
        !((width==1280 && height==720) || (width==1920 && height==1080) ||
          (width==2560 && height==1440) || (width==3840 && height==2160)))
        return PF2_UNSUPPORTED;
    if (fps != 60 && fps != 120) return PF2_INVALID;
    if (p->tv_width == width && p->tv_height == height && p->tv_fps == fps)
        return PF2_OK;
    p->tv_width=width;
    p->tv_height=height;
    p->tv_fps=fps;
    // Same lifetime as Moonlight NX2: the NWindow starts at a per-mode size
    // of 0x0 (follow TV) and remains registered. Switching refresh requests
    // is supported while streaming; changing the registered window dimensions
    // is not a physical HDMI mode setter and may fail with live buffers.
    pf2_apply_window_frame_rates(p);
    char policy[192];
    snprintf(policy,sizeof(policy),
             "presenter.tv.v2 docked=%u stream=%ux%u fps=%u rate_rc=0x%x",
             (unsigned)(appletGetOperationMode()==AppletOperationMode_Console),
             width,height,fps,p->refresh_policy_rc);
    pf2_runtime_boot_trace(policy);
    return PF2_OK;
}

static Pf2Error pf2_presenter_open_impl(Pf2Presenter **out, uint32_t width,
                                          uint32_t height, bool fixed_tv) {
    if (!out || width<2 || height<2 || width>8192 || height>8192) return PF2_INVALID;
    *out=NULL;
    Pf2Presenter *p=calloc(1,sizeof(*p));
    if (!p) return PF2_NO_MEMORY;
    p->tv_fps=60;
    const char *phase="window-policy";
    pf2_runtime_boot_trace(fixed_tv ? "presenter.open.fixed1080.begin" : "presenter.open.native.begin");
    // The default libounce window starts at 1280x720 for compatibility.
    // Native on Switch 2 must use the 1920x1080 handheld panel and, in
    // console mode, AM's actual TV-output setting (720p/1080p/1440p/4K).
    // A 0x0 docked size subscribes to the system's default-resolution event.
    NWindow *window=nwindowGetDefault();
    Result size_rc=nwindowSetModeDimensions(window,1920,1080,
                                            fixed_tv ? width : 0,
                                            fixed_tv ? height : 0);
    p->size_policy_rc=(uint32_t)size_rc;
    if (R_FAILED(size_rc) && !fixed_tv) {
        // HBMenu/applet variants may not expose the change event. Query the
        // current TV size directly before using a fixed 1080p fallback.
        s32 tv_w=0,tv_h=0;
        if (R_FAILED(appletGetDefaultDisplayResolution(&tv_w,&tv_h)) ||
            tv_w<2 || tv_h<2 || tv_w>3840 || tv_h>2160) {
            tv_w=1920;tv_h=1080;
        }
        if (R_FAILED(nwindowSetModeDimensions(window,1920,1080,
                                              (u32)tv_w,(u32)tv_h))) {
            pf2_runtime_boot_trace("presenter.window-policy.fallback.failed");
            free(p);
            return PF2_SDK_ERROR;
        }
    }
    if (R_FAILED(size_rc) && fixed_tv) {
        pf2_runtime_boot_trace("presenter.window-policy.fixed.failed");
        free(p);
        return PF2_SDK_ERROR;
    }
    u32 output_w=0,output_h=0;
    if (R_FAILED(nwindowGetDimensions(window,&output_w,&output_h)) ||
        output_w<2 || output_h<2 || output_w>3840 || output_h>2160) {
        pf2_runtime_boot_trace("presenter.window-dimensions.failed");
        free(p);
        return PF2_UNSUPPORTED;
    }
    p->desired_w=output_w;p->desired_h=output_h;
    char trace[160];
    snprintf(trace, sizeof(trace), "presenter.window %ux%u mode=%s policy=0x%x",
             output_w, output_h, fixed_tv ? "fixed" : "native", p->size_policy_rc);
    pf2_runtime_boot_trace(trace);
    // Keep the user's actual docked output mode. Requesting 120 Hz globally
    // is what moved 4K60 -> 1080p120 in the crash log.
    pf2_apply_window_frame_rates(p);
    const char *extensions[]={VK_KHR_SURFACE_EXTENSION_NAME,VK_NN_VI_SURFACE_EXTENSION_NAME};
    VkApplicationInfo ai={0};
    ai.sType=VK_STRUCTURE_TYPE_APPLICATION_INFO;ai.pApplicationName="Punktfunk Switch 2";
    ai.apiVersion=VK_API_VERSION_1_3;
    VkInstanceCreateInfo ii={0};
    ii.sType=VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
    ii.pApplicationInfo=&ai;ii.enabledExtensionCount=2;ii.ppEnabledExtensionNames=extensions;
    phase="vkCreateInstance";
    if (vkCreateInstance(&ii,NULL,&p->instance)!=VK_SUCCESS) goto failed;
    VkViSurfaceCreateInfoNN surface={0};
    surface.sType=VK_STRUCTURE_TYPE_VI_SURFACE_CREATE_INFO_NN;
    surface.window=nwindowGetDefault();
    phase="vkCreateViSurfaceNN";
    if (vkCreateViSurfaceNN(p->instance,&surface,NULL,&p->surface)!=VK_SUCCESS) goto failed;
    uint32_t count=0;
    if (vkEnumeratePhysicalDevices(p->instance,&count,NULL)!=VK_SUCCESS || !count) goto failed;
    VkPhysicalDevice *phys=calloc(count,sizeof(*phys));
    if (!phys) goto failed;
    VkResult rc=vkEnumeratePhysicalDevices(p->instance,&count,phys);
    if (rc==VK_SUCCESS) for (uint32_t i=0; i<count && !p->physical; i++) {
        uint32_t families=0;
        vkGetPhysicalDeviceQueueFamilyProperties(phys[i],&families,NULL);
        VkQueueFamilyProperties *props=calloc(families,sizeof(*props));
        if (!props) continue;
        vkGetPhysicalDeviceQueueFamilyProperties(phys[i],&families,props);
        for (uint32_t j=0;j<families;j++) {
            VkBool32 present=VK_FALSE;
            vkGetPhysicalDeviceSurfaceSupportKHR(phys[i],j,p->surface,&present);
            if ((props[j].queueFlags & VK_QUEUE_GRAPHICS_BIT) && present) {
                p->physical=phys[i];p->queue_family=j;break;
            }
        }
        free(props);
    }
    free(phys);
    phase="find-present-queue";
    if (!p->physical) goto failed;
    float priority=1.0f;
    VkDeviceQueueCreateInfo qi={0};
    qi.sType=VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
    qi.queueFamilyIndex=p->queue_family;qi.queueCount=1;qi.pQueuePriorities=&priority;
    const char *dev_ext[]={VK_KHR_SWAPCHAIN_EXTENSION_NAME};
    VkDeviceCreateInfo di={0};
    di.sType=VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
    di.queueCreateInfoCount=1;di.pQueueCreateInfos=&qi;
    di.enabledExtensionCount=1;di.ppEnabledExtensionNames=dev_ext;
    phase="vkCreateDevice";
    if (vkCreateDevice(p->physical,&di,NULL,&p->device)!=VK_SUCCESS) goto failed;
    vkGetDeviceQueue(p->device,p->queue_family,0,&p->queue);
    phase="create-swapchain";
    if (create_swap(p)!=PF2_OK) goto failed;
    // Only the legacy CPU fallback needs the mapped RGBA upload buffer.
    // Avoid reserving 32 MiB at 4K while Ganesh is active.
    phase="create-sync";
    if (frame_objects(p)!=PF2_OK) goto failed;
    *out=p;
    pf2_runtime_boot_trace("presenter.open.ok");
    return PF2_OK;
failed:
    snprintf(trace,sizeof(trace),"presenter.open.failed phase=%s window=%ux%u",
             phase,output_w,output_h);
    pf2_runtime_boot_trace(trace);
    pf2_presenter_close(p);
    return PF2_SDK_ERROR;
}

Pf2Error pf2_presenter_open(Pf2Presenter **out, uint32_t width, uint32_t height) {
    return pf2_presenter_open_impl(out, width, height, false);
}

// Emergency fallback only when opening the actual TV output fails. VI2 can
// scale the 1080p layer to the user's TV mode. Do not take this branch when
// native 4K presentation works: Native must retain the TV's real dimensions.
Pf2Error pf2_presenter_open_fixed(Pf2Presenter **out, uint32_t width, uint32_t height) {
    return pf2_presenter_open_impl(out, width, height, true);
}

Pf2Error pf2_presenter_gpu_image(Pf2Presenter *p, Pf2VkOffscreenImage *out) {
    if (!p || !out || !p->device || !p->swap || p->invalidated) return PF2_INVALID;
    if (!p->gpu_image) {
        VkFormatProperties properties;
        vkGetPhysicalDeviceFormatProperties(p->physical, p->format, &properties);
        if (!(properties.optimalTilingFeatures & VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT))
            return PF2_UNSUPPORTED;
        VkImageCreateInfo ci={0};
        ci.sType=VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO;
        ci.imageType=VK_IMAGE_TYPE_2D;
        ci.format=p->format;
        ci.extent=(VkExtent3D){p->extent.width,p->extent.height,1};
        ci.mipLevels=1;ci.arrayLayers=1;
        ci.samples=VK_SAMPLE_COUNT_1_BIT;
        ci.tiling=VK_IMAGE_TILING_OPTIMAL;
        // Skia Ganesh requires both TRANSFER_SRC and TRANSFER_DST on wrapped
        // VkImages (GrVkGpu::check_image_info); the native presenter still
        // only reads this offscreen image for VkCmdCopyImage.
        ci.usage=VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT|
                 VK_IMAGE_USAGE_TRANSFER_SRC_BIT|VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        ci.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
        ci.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED;
        if (vkCreateImage(p->device,&ci,NULL,&p->gpu_image)!=VK_SUCCESS)
            return PF2_UNSUPPORTED;
        VkMemoryRequirements req;
        vkGetImageMemoryRequirements(p->device,p->gpu_image,&req);
        uint32_t type=find_memory(p->physical,req.memoryTypeBits,VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        if (type==UINT32_MAX) type=find_memory(p->physical,req.memoryTypeBits,0);
        if (type==UINT32_MAX) goto failed;
        VkMemoryAllocateInfo ai={0};ai.sType=VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO;
        ai.allocationSize=req.size;ai.memoryTypeIndex=type;
        if (vkAllocateMemory(p->device,&ai,NULL,&p->gpu_memory)!=VK_SUCCESS) goto failed;
        if (vkBindImageMemory(p->device,p->gpu_image,p->gpu_memory,0)!=VK_SUCCESS) goto failed;
        p->gpu_format=p->format;
    }
    *out=(Pf2VkOffscreenImage){
        .image=(uint64_t)(uintptr_t)p->gpu_image,
        .format=(uint32_t)p->gpu_format,
        .width=p->extent.width,.height=p->extent.height,.queue_family=p->queue_family,
    };
    return PF2_OK;
failed:
    if (p->gpu_image) vkDestroyImage(p->device,p->gpu_image,NULL);
    if (p->gpu_memory) vkFreeMemory(p->device,p->gpu_memory,NULL);
    p->gpu_image=VK_NULL_HANDLE;p->gpu_memory=VK_NULL_HANDLE;
    return PF2_NO_MEMORY;
}

static void image_transition(VkCommandBuffer cmd,VkImage img,VkImageLayout before,
                             VkImageLayout after,VkAccessFlags from,VkAccessFlags to) {
    VkImageMemoryBarrier b={0};
    b.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    b.oldLayout=before;b.newLayout=after;
    b.srcAccessMask=from;b.dstAccessMask=to;
    b.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;
    b.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;
    b.image=img;
    b.subresourceRange.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;
    b.subresourceRange.levelCount=1;b.subresourceRange.layerCount=1;
    vkCmdPipelineBarrier(cmd,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT|VK_PIPELINE_STAGE_TRANSFER_BIT,
                         VK_PIPELINE_STAGE_TRANSFER_BIT|VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
                         0,0,NULL,0,NULL,1,&b);
}

// Both paths share one swapchain, VkQueue, fence, binary semaphore pair and
// command buffer. In the default configuration Ganesh submits with CPU sync;
// an opt-in async submission is ordered by the same Vulkan queue. Retain
// vkQueueWaitIdle after present until per-frame semaphore ownership exists.
static Pf2Error present_copy(Pf2Presenter *p, bool from_gpu) {
    if (p->invalidated) return PF2_OUT_OF_DATE;
    const uint32_t w=p->extent.width,h=p->extent.height;
    if (from_gpu && (!p->gpu_image || p->gpu_format!=p->format)) return PF2_OUT_OF_DATE;
    if (!p->fence || !p->acquire || !p->complete || p->device_lost)
        return PF2_SDK_ERROR;
    VkResult wait_rc=vkWaitForFences(p->device,1,&p->fence,VK_TRUE,2000000000ULL);
    if (wait_rc!=VK_SUCCESS) return broken_frame(p,wait_rc);
    uint32_t index=0;
    VkResult rc=vkAcquireNextImageKHR(p->device,p->swap,UINT64_MAX,p->acquire,VK_NULL_HANDLE,&index);
    // Surface mode changes must be serviced *after* Rust has dropped the
    // Ganesh VkImage wrapper. Leave swapchain and device owned by p intact.
    if (rc==VK_ERROR_OUT_OF_DATE_KHR || rc==VK_SUBOPTIMAL_KHR) {
        p->invalidated=true;
        return PF2_OUT_OF_DATE;
    }
    if (rc!=VK_SUCCESS || index>=p->image_count) return broken_frame(p,rc);
    rc=vkResetCommandBuffer(p->cmd,0);
    if (rc!=VK_SUCCESS) return broken_frame(p,rc);
    VkCommandBufferBeginInfo bi={0};bi.sType=VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO;
    bi.flags=VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
    rc=vkBeginCommandBuffer(p->cmd,&bi);
    if (rc!=VK_SUCCESS) return broken_frame(p,rc);
    if (from_gpu) {
        // No layout transition here: Ganesh explicitly flushed into
        // TRANSFER_SRC_OPTIMAL. This memory barrier makes writes visible to copy.
        VkImageMemoryBarrier barrier={0};
        barrier.sType=VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
        barrier.srcAccessMask=VK_ACCESS_MEMORY_WRITE_BIT;
        barrier.dstAccessMask=VK_ACCESS_TRANSFER_READ_BIT;
        barrier.oldLayout=VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
        barrier.newLayout=VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
        barrier.srcQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;
        barrier.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;
        barrier.image=p->gpu_image;
        barrier.subresourceRange.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;
        barrier.subresourceRange.levelCount=1;
        barrier.subresourceRange.layerCount=1;
        vkCmdPipelineBarrier(p->cmd,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
                             VK_PIPELINE_STAGE_TRANSFER_BIT,0,0,NULL,0,NULL,1,&barrier);
    }
    image_transition(p->cmd,p->images[index],VK_IMAGE_LAYOUT_UNDEFINED,
                     VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,0,VK_ACCESS_TRANSFER_WRITE_BIT);
    if (from_gpu) {
        VkImageCopy region={0};
        region.srcSubresource.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;
        region.srcSubresource.layerCount=1;
        region.dstSubresource.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;
        region.dstSubresource.layerCount=1;
        region.extent=(VkExtent3D){w,h,1};
        vkCmdCopyImage(p->cmd,p->gpu_image,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
                       p->images[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,1,&region);
    } else {
        VkBufferImageCopy region={0};
        region.imageSubresource.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;
        region.imageSubresource.layerCount=1;
        region.imageExtent=(VkExtent3D){w,h,1};
        vkCmdCopyBufferToImage(p->cmd,p->upload,p->images[index],
                               VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,1,&region);
    }
    image_transition(p->cmd,p->images[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                     VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,VK_ACCESS_TRANSFER_WRITE_BIT,0);
    rc=vkEndCommandBuffer(p->cmd);
    if (rc!=VK_SUCCESS) return broken_frame(p,rc);
    rc=vkResetFences(p->device,1,&p->fence);
    if (rc!=VK_SUCCESS) return broken_frame(p,rc);
    VkPipelineStageFlags stage=VK_PIPELINE_STAGE_TRANSFER_BIT;
    VkSubmitInfo submit={0};submit.sType=VK_STRUCTURE_TYPE_SUBMIT_INFO;
    submit.waitSemaphoreCount=1;submit.pWaitSemaphores=&p->acquire;
    submit.pWaitDstStageMask=&stage;submit.commandBufferCount=1;submit.pCommandBuffers=&p->cmd;
    submit.signalSemaphoreCount=1;submit.pSignalSemaphores=&p->complete;
    rc=vkQueueSubmit(p->queue,1,&submit,p->fence);
    if (rc!=VK_SUCCESS) return broken_frame(p,rc);
    VkPresentInfoKHR present={0};present.sType=VK_STRUCTURE_TYPE_PRESENT_INFO_KHR;
    present.waitSemaphoreCount=1;present.pWaitSemaphores=&p->complete;
    present.swapchainCount=1;present.pSwapchains=&p->swap;present.pImageIndices=&index;
    rc=vkQueuePresentKHR(p->queue,&present);
    // Keep the queue idle barrier until a multi-frame sync design exists:
    // presentation may still own the complete semaphore on the next tick.
    VkResult idle_rc=vkQueueWaitIdle(p->queue);
    if (idle_rc!=VK_SUCCESS) return broken_frame(p,idle_rc);
    if (rc==VK_ERROR_OUT_OF_DATE_KHR || rc==VK_SUBOPTIMAL_KHR) {
        p->invalidated=true; return PF2_OUT_OF_DATE;
    }
    return rc==VK_SUCCESS ? PF2_OK:broken_frame(p,rc);
}

Pf2Error pf2_presenter_frame_gpu(Pf2Presenter *p) {
    if (!p || !p->device || !p->swap || !p->gpu_image) return PF2_INVALID;
    return present_copy(p,true);
}

Pf2Error pf2_presenter_frame(Pf2Presenter *p,const uint8_t *rgba,size_t bytes,size_t stride) {
    if (!p || !rgba || !p->device || !p->swap) return PF2_INVALID;
    if (p->invalidated) return PF2_OUT_OF_DATE;
    if (!p->mapped) {
        Pf2Error upload_rc=open_upload(p);
        if (upload_rc!=PF2_OK) return upload_rc;
    }
    const uint32_t w=p->extent.width,h=p->extent.height;
    if (w<2 || h<2 || stride < (size_t)w*4 ||
        stride > SIZE_MAX / (size_t)(h-1) ||
        bytes < stride*(size_t)(h-1)+(size_t)w*4)
        return PF2_INVALID;
    // Preserve the original CPU upload-buffer fence before writing mapped memory.
    if (!p->fence || p->device_lost) return PF2_SDK_ERROR;
    VkResult upload_wait=vkWaitForFences(p->device,1,&p->fence,VK_TRUE,2000000000ULL);
    if (upload_wait!=VK_SUCCESS) return broken_frame(p,upload_wait);
    for (uint32_t y=0;y<h;y++) {
        const uint8_t *src=rgba+(size_t)y*stride;
        uint8_t *dst=(uint8_t*)p->mapped+(size_t)y*w*4;
        if (p->format==VK_FORMAT_R8G8B8A8_UNORM) memcpy(dst,src,(size_t)w*4);
        else for (uint32_t x=0;x<w;x++) {
            dst[x*4+0]=src[x*4+2];dst[x*4+1]=src[x*4+1];
            dst[x*4+2]=src[x*4+0];dst[x*4+3]=src[x*4+3];
        }
    }
    if (!p->host_coherent) {
        VkMappedMemoryRange range={0};range.sType=VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE;
        range.memory=p->upload_memory;range.offset=0;range.size=VK_WHOLE_SIZE;
        VkResult flush_rc=vkFlushMappedMemoryRanges(p->device,1,&range);
        if (flush_rc!=VK_SUCCESS) return broken_frame(p,flush_rc);
    }
    return present_copy(p,false);
}

// Reconfigure on the *same* VkInstance/VkDevice/VI surface. The Rust caller
// drops its Ganesh Surface and DirectContext before this call, and recreates
// those wrappers only after receiving the new extent. Moonlight NX2 follows
// the same lifetime rule for swapchain changes (oldSwapchain + wait_all).
Pf2Error pf2_presenter_resize(Pf2Presenter *p) {
    if (!p || !p->device || !p->swap) return PF2_INVALID;
    if (p->device_lost) return PF2_SDK_ERROR;
    VkResult idle_rc=vkDeviceWaitIdle(p->device);
    if (idle_rc!=VK_SUCCESS) return broken_frame(p,idle_rc);
    pf2_apply_window_frame_rates(p);
    u32 w=0,h=0;
    if (R_SUCCEEDED(nwindowGetDimensions(nwindowGetDefault(),&w,&h)) &&
        w>=2 && h>=2 && w<=3840 && h<=2160) {
        p->desired_w=w;
        p->desired_h=h;
    }
    const uint32_t old_w=p->extent.width, old_h=p->extent.height;
    const VkFormat old_format=p->format;
    Pf2Error result=create_swap(p);
    if (result!=PF2_OK) {
        p->invalidated=true;
        pf2_runtime_boot_trace("presenter.resize.swapchain.failed");
        return result;
    }
    // No Skia reference to the old offscreen image exists at this point.
    if (p->gpu_image) vkDestroyImage(p->device,p->gpu_image,NULL);
    if (p->gpu_memory) vkFreeMemory(p->device,p->gpu_memory,NULL);
    p->gpu_image=VK_NULL_HANDLE;
    p->gpu_memory=VK_NULL_HANDLE;
    p->gpu_format=VK_FORMAT_UNDEFINED;
    // The lazy raster upload is tied to the old image extent/format.
    close_upload(p);
    // Also replace the fence: it may have been reset by a failed submit.
    // vkDeviceWaitIdle above is essential before freeing these resources.
    if (renew_frame_sync(p)!=PF2_OK) {
        p->invalidated=true;
        pf2_runtime_boot_trace("presenter.resize.sync.failed");
        return PF2_SDK_ERROR;
    }
    p->invalidated=false;
    char trace[160];
    snprintf(trace,sizeof(trace),"presenter.resize.ok %ux%u -> %ux%u format=%u->%u",
             old_w,old_h,p->extent.width,p->extent.height,
             (unsigned)old_format,(unsigned)p->format);
    pf2_runtime_boot_trace(trace);
    return PF2_OK;
}

void pf2_presenter_close(Pf2Presenter *p) {
    if (!p) return;
    if (p->device) vkDeviceWaitIdle(p->device);
    // SkSurface/DirectContext are dropped by the Rust owner before this call.
    if (p->gpu_image) vkDestroyImage(p->device,p->gpu_image,NULL);
    if (p->gpu_memory) vkFreeMemory(p->device,p->gpu_memory,NULL);
    if (p->device) close_upload(p);
    if (p->fence) vkDestroyFence(p->device,p->fence,NULL);
    if (p->complete) vkDestroySemaphore(p->device,p->complete,NULL);
    if (p->acquire) vkDestroySemaphore(p->device,p->acquire,NULL);
    if (p->pool) vkDestroyCommandPool(p->device,p->pool,NULL);
    if (p->swap) vkDestroySwapchainKHR(p->device,p->swap,NULL);
    if (p->device) vkDestroyDevice(p->device,NULL);
    if (p->surface) vkDestroySurfaceKHR(p->instance,p->surface,NULL);
    if (p->instance) vkDestroyInstance(p->instance,NULL);
    free(p);
}

Pf2Error pf2_presenter_vk_borrow(const Pf2Presenter *p, Pf2VkBorrowedContext *out) {
    if (!p || !out || !p->instance || !p->physical || !p->device || !p->queue)
        return PF2_INVALID;
    VkPhysicalDeviceProperties props;
    vkGetPhysicalDeviceProperties(p->physical, &props);
    out->instance = (void *)p->instance;
    out->physical_device = (void *)p->physical;
    out->device = (void *)p->device;
    out->queue = (void *)p->queue;
    out->queue_family = p->queue_family;
    out->physical_api_version = props.apiVersion;
    return PF2_OK;
}

const void *pf2_vk_instance_proc(void *instance, const char *name) {
    if (!name) return NULL;
    // A null instance is valid for Vulkan global entry points.
    return (const void *)vkGetInstanceProcAddr((VkInstance)instance, name);
}

const void *pf2_vk_device_proc(void *device, const char *name) {
    if (!device || !name) return NULL;
    return (const void *)vkGetDeviceProcAddr((VkDevice)device, name);
}

Pf2Error pf2_presenter_size(const Pf2Presenter *p, uint32_t *width, uint32_t *height) {
    if (!p || !width || !height || !p->swap) return PF2_INVALID;
    *width = p->extent.width;
    *height = p->extent.height;
    return PF2_OK;
}

// Live native-window dimensions; nwindowGetDimensions refreshes the
// per-mode policy and AM's default-resolution event on each call.
Pf2Error pf2_presenter_output_size(const Pf2Presenter *p, uint32_t *width,
                                    uint32_t *height, uint32_t *policy_rc) {
    if (!p || !width || !height || !policy_rc) return PF2_INVALID;
    *policy_rc=p->size_policy_rc;
    if (R_FAILED(nwindowGetDimensions(nwindowGetDefault(),width,height)))
        return PF2_SDK_ERROR;
    if (*width<2 || *height<2 || *width>3840 || *height>2160)
        return PF2_UNSUPPORTED;
    return PF2_OK;
}

Pf2Error pf2_presenter_refresh(const Pf2Presenter *p, uint32_t *out_hz, uint32_t *out_policy_rc) {
    if (!p || !out_hz || !out_policy_rc) return PF2_INVALID;
    *out_hz=60;
    *out_policy_rc=p->refresh_policy_rc;
    Vi2Display *display=NULL;
    Result rc=nwindowGetVi2Display(nwindowGetDefault(),&display);
    if (R_FAILED(rc) || !display) return PF2_SDK_ERROR;
    u32 hz=0;
    rc=vi2QueryDisplayRefreshRate(display,&hz,NULL);
    if (R_FAILED(rc) || (hz!=60 && hz!=120)) return PF2_SDK_ERROR;
    *out_hz=hz;
    return PF2_OK;
}
