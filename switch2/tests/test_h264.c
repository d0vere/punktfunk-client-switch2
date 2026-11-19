#include "../native/pf2_native.h"
#include <assert.h>
#include <string.h>
#include <stdio.h>
int main(void) {
    const uint8_t annexb[] = {
        0,0,1,0x65,0x80, 0,0,0,1,0x68,0xcc,0x10, 0,0,1,0x67,0x42,0x00,0x1e,
        0,0,1,0x41,0x10
    };
    const uint8_t expected[] = {
        0,0,0,1,0x67,0x42,0x00,0x1e, 0,0,0,1,0x68,0xcc,0x10
    };
    uint8_t output[64] = {0}; size_t written = 999;
    assert(pf2_h264_config_from_annexb(annexb,sizeof annexb,output,sizeof output,&written)==PF2_OK);
    assert(written == sizeof expected && !memcmp(output,expected,written));
    written=123;
    assert(pf2_h264_config_from_annexb(annexb,sizeof annexb,output,10,&written)==PF2_NO_MEMORY);
    assert(written==0);
    const uint8_t missing[] = {0,0,1,0x67,0x42};
    assert(pf2_h264_config_from_annexb(missing,sizeof missing,output,sizeof output,&written)==PF2_NOT_READY);
    puts("PASS H264 Annex B parameter set extraction");
    return 0;
}
