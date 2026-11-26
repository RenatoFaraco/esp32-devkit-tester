/*
 * Hello firmware: prints the chip's own view of itself.
 *
 * Where the data comes from, in hardware terms:
 * - Silicon revision: eFuse bits burned at the factory (WAFER_VERSION major/minor), read by
 *   esp_chip_info() from the eFuse controller's shadow registers.
 * - Flash size: the SPI flash controller issues the JEDEC "Read ID" command (0x9F) to the external
 *   (or in-package) flash chip; the capacity byte of that ID gives the size.
 * - Model, cores and features: fixed per die / package, also derived from eFuse and SoC data.
 */
#include <inttypes.h>
#include <stdint.h>

#include "esp_chip_info.h"
#include "esp_err.h"
#include "esp_flash.h"
#include "esp_log.h"
#include "sdkconfig.h"

static const char *const DKT_TAG = "DKT_MAIN";

static const char *dkt_chip_model_name(esp_chip_model_t model) {
    switch (model) {
        case CHIP_ESP32:
            return "ESP32";
        case CHIP_ESP32S2:
            return "ESP32-S2";
        case CHIP_ESP32S3:
            return "ESP32-S3";
        case CHIP_ESP32C3:
            return "ESP32-C3";
        case CHIP_ESP32C2:
            return "ESP32-C2";
        case CHIP_ESP32C5:
            return "ESP32-C5";
        case CHIP_ESP32C6:
            return "ESP32-C6";
        case CHIP_ESP32C61:
            return "ESP32-C61";
        case CHIP_ESP32H2:
            return "ESP32-H2";
        case CHIP_ESP32P4:
            return "ESP32-P4";
        default:
            return "unknown";
    }
}

static const char *dkt_yes_no(uint32_t features, uint32_t bit) {
    return (features & bit) != 0U ? "yes" : "no";
}

static void dkt_print_chip_info(void) {
    esp_chip_info_t info;
    esp_chip_info(&info);

    ESP_LOGI(DKT_TAG, "build target: %s", CONFIG_IDF_TARGET);
    ESP_LOGI(DKT_TAG, "chip model:   %s (%d)", dkt_chip_model_name(info.model), (int)info.model);
    ESP_LOGI(DKT_TAG, "cores:        %u", (unsigned)info.cores);
    /* revision is encoded as major * 100 + minor */
    ESP_LOGI(DKT_TAG, "revision:     v%u.%u", (unsigned)(info.revision / 100U),
             (unsigned)(info.revision % 100U));
    ESP_LOGI(DKT_TAG, "wifi 2.4 GHz: %s", dkt_yes_no(info.features, CHIP_FEATURE_WIFI_BGN));
    ESP_LOGI(DKT_TAG, "bluetooth LE: %s", dkt_yes_no(info.features, CHIP_FEATURE_BLE));
    ESP_LOGI(DKT_TAG, "bt classic:   %s", dkt_yes_no(info.features, CHIP_FEATURE_BT));
    ESP_LOGI(DKT_TAG, "802.15.4:     %s", dkt_yes_no(info.features, CHIP_FEATURE_IEEE802154));
    ESP_LOGI(DKT_TAG, "emb. flash:   %s", dkt_yes_no(info.features, CHIP_FEATURE_EMB_FLASH));
    ESP_LOGI(DKT_TAG, "emb. psram:   %s", dkt_yes_no(info.features, CHIP_FEATURE_EMB_PSRAM));
}

static void dkt_print_flash_size(void) {
    uint32_t size = 0;
    const esp_err_t err = esp_flash_get_size(NULL, &size);
    if (err != ESP_OK) {
        ESP_LOGE(DKT_TAG, "esp_flash_get_size failed: %s", esp_err_to_name(err));
        return;
    }
    ESP_LOGI(DKT_TAG, "flash size:   %" PRIu32 " bytes (%" PRIu32 " MB)", size,
             size / (1024U * 1024U));
}

void app_main(void) {
    dkt_print_chip_info();
    dkt_print_flash_size();
}
