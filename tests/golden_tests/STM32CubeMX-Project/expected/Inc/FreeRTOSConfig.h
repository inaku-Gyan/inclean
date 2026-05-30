#ifndef FREERTOS_CONFIG_H
#define FREERTOS_CONFIG_H

#if defined(__ICCARM__) || defined(__CC_ARM) || defined(__GNUC__)
  #include <stdint.h>
#endif
#ifndef CMSIS_device_header
#define CMSIS_device_header "../../../../../Drivers/CMSIS/Device/ST/STM32F4xx/Include/stm32f4xx.h"
#endif /* CMSIS_device_header */

#endif /* FREERTOS_CONFIG_H */
