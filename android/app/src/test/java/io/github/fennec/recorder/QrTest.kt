package io.github.fennec.recorder

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeWriter
import io.github.fennec.recorder.net.ScannedCode
import io.github.fennec.recorder.net.SendTarget
import io.github.fennec.recorder.net.SendUri
import org.junit.Assert.assertEquals
import org.junit.Test
import java.awt.image.BufferedImage
import javax.imageio.ImageIO

/** Codes as the camera reads them, with the same decoder. */
class QrTest {
    private fun decode(img: BufferedImage): String {
        val pixels = IntArray(img.width * img.height).also { img.getRGB(0, 0, img.width, img.height, it, 0, img.width) }
        val bitmap = BinaryBitmap(HybridBinarizer(RGBLuminanceSource(img.width, img.height, pixels)))
        return MultiFormatReader().decode(
            bitmap,
            mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE), DecodeHintType.TRY_HARDER to true),
        ).text
    }

    @Test
    fun `a send code is read and understood`() {
        val pin = "ODl9kE1pZ0yJ4o3b8zq5H2wTn7cVfR6sXuAaQeLgMtY"
        val link = "fennec://send?h=192.168.1.20:47130&pin=$pin&t=q8Zr3kVb1sXn0YtLm4Pw2A&n=Ane%27s%20laptop"
        val matrix = QRCodeWriter().encode(link, BarcodeFormat.QR_CODE, 400, 400)
        val img = BufferedImage(matrix.width, matrix.height, BufferedImage.TYPE_INT_RGB)
        for (x in 0 until matrix.width) for (y in 0 until matrix.height) img.setRGB(x, y, if (matrix[x, y]) 0 else 0xFFFFFF)
        assertEquals(
            ScannedCode.Send(SendTarget("192.168.1.20:47130", pin, "q8Zr3kVb1sXn0YtLm4Pw2A", "Ane's laptop")),
            SendUri.read(decode(img)),
        )
    }

    /** A screenshot of Settings → Phone from an older Fennec, which paired. */
    @Test
    fun `an older Fennec's pairing code is recognised as one`() {
        val text = decode(ImageIO.read(javaClass.getResourceAsStream("/desktop-pairing.png")))
        assertEquals(ScannedCode.OldFennec, SendUri.read(text))
    }
}
