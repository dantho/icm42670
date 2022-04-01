//! An [embedded-hal] driver for the ICM-42670 6-axis IMU from InvenSense.
//!
//! The ICM-42670 combines a 3-axis accelerometer with a 3-axis gyroscope into a
//! single package. It has a configurable host interface which supports I²C,
//! SPI, and I3C communications. Presently this driver only supports using the
//! I²C interface.
//!
//! For additional information about this device please refer to the
//! [datasheet].
//!
//! [embedded-hal]: https://docs.rs/embedded-hal/latest/embedded_hal/
//! [datasheet]: https://3cfeqx1hf82y3xcoull08ihx-wpengine.netdna-ssl.com/wp-content/uploads/2021/07/DS-000451-ICM-42670-P-v1.0.pdf

#![no_std]

use core::fmt::Debug;

pub use accelerometer;
use accelerometer::{
    error::Error as AccelerometerError,
    vector::{F32x3, U16x3},
    Accelerometer,
    RawAccelerometer,
};
use embedded_hal::blocking::{
    delay::DelayUs,
    i2c::{Write, WriteRead},
};

use crate::{
    config::Bitfield,
    register::{Bank0, Mreg1, Mreg2, Mreg3, Register, RegisterBank},
};
pub use crate::{
    config::{AccelRange, Address, GyroRange, PowerMode},
    error::Error,
};

mod config;
mod error;
mod register;

/// Re-export any traits which may be required by end users
pub mod prelude {
    pub use accelerometer::{Accelerometer as _, RawAccelerometer as _};
}

/// ICM-42670 driver
#[derive(Debug, Clone, Copy)]
pub struct Icm42670<I2C> {
    /// Underlying I²C peripheral
    i2c: I2C,
    /// I²C slave address to use
    address: Address,
}

impl<I2C, E> Icm42670<I2C>
where
    I2C: Write<Error = E> + WriteRead<Error = E>,
    Error<E>: From<Error<()>>,
{
    /// Unique device identifier for the ICM-42670
    pub const WHO_AM_I: u8 = 0x67;

    pub fn new(i2c: I2C, address: Address) -> Result<Self, Error<E>> {
        let mut me = Self { i2c, address };
        if me.device_id()? != Self::WHO_AM_I {
            return Err(Error::BadChip);
        }

        me.set_power_mode(PowerMode::SixAxisLowNoise)?;
        me.set_accel_range(AccelRange::G4)?;
        me.set_gyro_range(GyroRange::Deg2000)?;

        Ok(me)
    }

    /// Return the raw interface to the underlying `I2C` instance
    pub fn free(self) -> I2C {
        self.i2c
    }

    /// Read the ID of the connected device
    pub fn device_id(&mut self) -> Result<u8, Error<E>> {
        self.read_reg(&Bank0::WHO_AM_I)
    }

    /// Read the raw gyro data for each of the three axes
    pub fn gyro_raw(&mut self) -> Result<U16x3, Error<E>> {
        let x = self.read_reg_u16(&Bank0::GYRO_DATA_X1, &Bank0::GYRO_DATA_X0)?;
        let y = self.read_reg_u16(&Bank0::GYRO_DATA_Y1, &Bank0::GYRO_DATA_Y0)?;
        let z = self.read_reg_u16(&Bank0::GYRO_DATA_Z1, &Bank0::GYRO_DATA_Z0)?;

        Ok(U16x3::new(x, y, z))
    }

    pub fn gyro_norm(&mut self) -> Result<F32x3, Error<E>> {
        todo!()
    }

    /// Read the built-in temperature sensor and return the value in degrees
    /// centigrade
    pub fn temperature(&mut self) -> Result<f32, Error<E>> {
        let raw = self.temperature_raw()? as f32;
        let deg = (raw / 128.0) + 25.0;

        Ok(deg)
    }

    /// Read the raw data from the built-in temperature sensor
    pub fn temperature_raw(&mut self) -> Result<u16, Error<E>> {
        self.read_reg_u16(&Bank0::TEMP_DATA1, &Bank0::TEMP_DATA0)
    }

    /// Return the currently configured power mode
    pub fn power_mode(&mut self) -> Result<PowerMode, Error<E>> {
        todo!()
    }

    /// Set the power mode of the IMU
    pub fn set_power_mode(&mut self, mode: PowerMode) -> Result<(), Error<E>> {
        self.update_reg(&Bank0::PWR_MGMT0, mode.bits(), PowerMode::BITMASK)
    }

    /// Return the currently configured accelerometer range
    pub fn accel_range(&mut self) -> Result<AccelRange, Error<E>> {
        // `ACCEL_UI_FS_SEL` occupies bits 6:5 in the register
        let fs_sel = self.read_reg(&Bank0::ACCEL_CONFIG0)? >> 5;
        let range = AccelRange::try_from(fs_sel)?;

        Ok(range)
    }

    /// Set the range of the accelerometer
    pub fn set_accel_range(&mut self, range: AccelRange) -> Result<(), Error<E>> {
        self.update_reg(&Bank0::ACCEL_CONFIG0, range.bits(), AccelRange::BITMASK)
    }

    /// Return the currently configured gyroscope range
    pub fn gyro_range(&mut self) -> Result<GyroRange, Error<E>> {
        // `GYRO_UI_FS_SEL` occupies bits 6:5 in the register
        let fs_sel = self.read_reg(&Bank0::GYRO_CONFIG0)? >> 5;
        let range = GyroRange::try_from(fs_sel)?;

        Ok(range)
    }

    /// Set the range of the gyro
    pub fn set_gyro_range(&mut self, range: GyroRange) -> Result<(), Error<E>> {
        self.update_reg(&Bank0::GYRO_CONFIG0, range.bits(), GyroRange::BITMASK)
    }

    // -----------------------------------------------------------------------
    // PRIVATE

    // FIXME: 'Sleep mode' and 'accelerometer low power mode with WUOSC' do not
    //        support MREG1, MREG2 or MREG3 access.
    fn read_mreg(
        &mut self,
        delay: &mut dyn DelayUs<u8>,
        bank: RegisterBank,
        reg: &dyn Register,
    ) -> Result<u8, Error<E>> {
        // See "ACCESSING MREG1, MREG2 AND MREG3 REGISTERS" (page 40)

        // Wait until the internal clock is running prior to writing.
        while self.read_reg(&Bank0::MCLK_RDY)? != 0x1 {}

        // Select the appropriate block and set the register address to read from.
        self.write_reg(&Bank0::BLK_SEL_R, bank.blk_sel())?;
        self.write_reg(&Bank0::MADDR_R, reg.addr())?;
        delay.delay_us(10);

        // Read a value from the register.
        let result = self.read_reg(&Bank0::M_R)?;
        delay.delay_us(10);

        // Reset block selection registers.
        self.write_reg(&Bank0::BLK_SEL_R, 0x00)?;
        self.write_reg(&Bank0::BLK_SEL_W, 0x00)?;

        Ok(result)
    }

    // FIXME: 'Sleep mode' and 'accelerometer low power mode with WUOSC' do not
    //        support MREG1, MREG2 or MREG3 access.
    fn write_mreg(
        &mut self,
        delay: &mut dyn DelayUs<u8>,
        bank: RegisterBank,
        reg: &dyn Register,
        value: u8,
    ) -> Result<(), Error<E>> {
        // See "ACCESSING MREG1, MREG2 AND MREG3 REGISTERS" (page 40)

        // Wait until the internal clock is running prior to writing.
        while self.read_reg(&Bank0::MCLK_RDY)? != 0x1 {}

        // Select the appropriate block and set the register address to write to.
        self.write_reg(&Bank0::BLK_SEL_W, bank.blk_sel())?;
        self.write_reg(&Bank0::MADDR_W, reg.addr())?;

        // Write the value to the register.
        self.write_reg(&Bank0::M_W, value)?;
        delay.delay_us(10);

        // Reset block selection registers.
        self.write_reg(&Bank0::BLK_SEL_R, 0x00)?;
        self.write_reg(&Bank0::BLK_SEL_W, 0x00)?;

        Ok(())
    }

    /// Read a register at the provided address.
    fn read_reg(&mut self, reg: &dyn Register) -> Result<u8, Error<E>> {
        let mut buffer = [0u8];
        self.i2c
            .write_read(self.address as u8, &[reg.addr()], &mut buffer)?;

        Ok(buffer[0])
    }

    /// Read two registers and combine them into a single value.
    fn read_reg_u16(
        &mut self,
        reg_hi: &dyn Register,
        reg_lo: &dyn Register,
    ) -> Result<u16, Error<E>> {
        let data_hi = self.read_reg(reg_hi)? as u16;
        let data_lo = self.read_reg(reg_lo)? as u16;

        let data = (data_hi << 8) | data_lo;

        Ok(data)
    }

    /// Set a register at the provided address to a given value.
    fn write_reg(&mut self, reg: &dyn Register, value: u8) -> Result<(), Error<E>> {
        if reg.read_only() {
            Err(Error::WriteToReadOnly)
        } else {
            self.i2c.write(self.address as u8, &[reg.addr(), value])?;
            Ok(())
        }
    }

    /// Update the register at the provided address.
    ///
    /// Rather than overwriting any active bits in the register, we first read
    /// in its current value and then update it accordingly using the given
    /// value and mask before writing back the desired value.
    fn update_reg(&mut self, reg: &dyn Register, value: u8, mask: u8) -> Result<(), Error<E>> {
        let current = self.read_reg(reg)?;
        let value = (current & !mask) | (value & mask);

        self.write_reg(reg, value)
    }
}

impl<I2C, E> Accelerometer for Icm42670<I2C>
where
    I2C: Write<Error = E> + WriteRead<Error = E>,
    E: Debug,
    Error<E>: From<Error<()>>,
{
    type Error = Error<E>;

    fn accel_norm(&mut self) -> Result<F32x3, AccelerometerError<Self::Error>> {
        let scale = 1f32; // FIXME: determine scale from mode/range
        let shift = 0u16; // FIXME: determine shift from mode

        let raw = self.accel_raw()?;

        let x = (raw.x >> shift) as f32 * scale;
        let y = (raw.y >> shift) as f32 * scale;
        let z = (raw.z >> shift) as f32 * scale;

        Ok(F32x3::new(x, y, z))
    }

    fn sample_rate(&mut self) -> Result<f32, AccelerometerError<Self::Error>> {
        todo!()
    }
}

impl<I2C, E> RawAccelerometer<U16x3> for Icm42670<I2C>
where
    I2C: Write<Error = E> + WriteRead<Error = E>,
    E: Debug,
    Error<E>: From<Error<()>>,
{
    type Error = Error<E>;

    fn accel_raw(&mut self) -> Result<U16x3, AccelerometerError<Self::Error>> {
        let x = self.read_reg_u16(&Bank0::ACCEL_DATA_X1, &Bank0::ACCEL_DATA_X0)?;
        let y = self.read_reg_u16(&Bank0::ACCEL_DATA_Y1, &Bank0::ACCEL_DATA_Y0)?;
        let z = self.read_reg_u16(&Bank0::ACCEL_DATA_Z1, &Bank0::ACCEL_DATA_Z0)?;

        Ok(U16x3::new(x, y, z))
    }
}
